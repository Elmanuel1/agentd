use shared::hash::content_sha256;
use shared::limits::{required, validate_bytes_max, validate_sha256_hex, values};
use shared::{ApiError, AuthContext, ErrorType, Timestamp};

use super::{map_store, parse_mem_pair, parse_store_id, MemoriesService};
use crate::store::{MemoryRecord, VersionRecord, VersionSelector};
use crate::types::{
    CreateMemoryRequest, Memory, MemoryId, MemoryTag, MemoryVersion, Precondition, UpdateMemoryRequest,
    VersionOperation,
};

impl MemoriesService {
    /// `POST /v1/memory_stores/{id}/memories` — 409 on occupied path.
    pub async fn create_memory(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        req: CreateMemoryRequest,
    ) -> Result<Memory, ApiError> {
        let store_id = parse_store_id(raw_store_id)?;
        let store = self.store.get_store(&ctx.workspace, &store_id).await.map_err(map_store)?;
        if store.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request("cannot write to an archived memory store"));
        }
        let path = required("path", req.path)?;
        validate_path(&path)?;
        let content = required("content", req.content)?;
        validate_bytes_max("content", content.len(), values::MEMORY_CONTENT_MAX_BYTES)?;
        let now = Timestamp::now();
        let mem_id = MemoryId::generate();
        let version = MemoryVersion::record(
            ctx.actor(),
            mem_id.clone(),
            store_id.clone(),
            VersionOperation::Created,
            path.clone(),
            Some(&content),
            now,
        );
        let head = Memory {
            content: None, // live: create response does NOT echo content
            content_sha256: version.content_sha256.clone().unwrap_or_default(),
            content_size_bytes: content.len() as u64,
            created_at: now,
            id: mem_id,
            memory_store_id: store_id.clone(),
            memory_version_id: version.id.clone(),
            path,
            tag: MemoryTag::Memory,
            updated_at: now,
        };
        let record = MemoryRecord {
            head: head.clone(),
            deleted: false,
            chain: vec![VersionRecord { version, blob: Some(content) }],
        };
        self.store.insert_memory(&ctx.workspace, &store_id, record).await.map_err(map_store)?;
        Ok(head)
    }

    /// `POST /v1/memory_stores/{sid}/memories/{mid}` — update via POST
    /// (live-verified; PATCH → 405 at the router). Appends a version and
    /// advances the head (`latest`).
    pub async fn update_memory(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        raw_mem_id: &str,
        req: UpdateMemoryRequest,
    ) -> Result<Memory, ApiError> {
        let (sid, mid) = parse_mem_pair(raw_store_id, raw_mem_id)?;
        let store = self.store.get_store(&ctx.workspace, &sid).await.map_err(map_store)?;
        if store.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request("cannot write to an archived memory store"));
        }
        let mut record = self.store.get_memory(&ctx.workspace, &sid, &mid).await.map_err(map_store)?;
        let new_content = match req.content {
            Some(c) => {
                validate_bytes_max("content", c.len(), values::MEMORY_CONTENT_MAX_BYTES)?;
                c
            }
            None => record.resolve(&VersionSelector::Latest).and_then(|v| v.blob.clone()).unwrap_or_default(),
        };
        let new_path = match req.path {
            Some(p) => {
                validate_path(&p)?;
                p
            }
            None => record.head.path.clone(),
        };
        // Live: an update that changes nothing writes no version, and that
        // holds even when its precondition names other content.
        if content_sha256(new_content.as_bytes()) == record.head.content_sha256
            && new_path == record.head.path
        {
            return Ok(record.head);
        }
        check_precondition(&record, req.precondition.as_ref())?;
        let now = Timestamp::now();
        let version = MemoryVersion::record(
            ctx.actor(),
            mid.clone(),
            sid.clone(),
            VersionOperation::Modified,
            new_path.clone(),
            Some(&new_content),
            now,
        );
        let (ver_id, sha) = (version.id.clone(), version.content_sha256.clone().unwrap_or_default());
        record.chain.push(VersionRecord { version, blob: Some(new_content) });
        // Advance the `latest` ref.
        record.head.memory_version_id = ver_id;
        record.head.content_sha256 = sha;
        record.head.content_size_bytes =
            record.chain.last().and_then(|v| v.version.content_size_bytes).unwrap_or_default();
        record.head.path = new_path;
        record.head.updated_at = now;
        let head = record.head.clone();
        self.store.replace_memory(&ctx.workspace, &sid, record).await.map_err(map_store)?;
        Ok(head)
    }

    /// `DELETE /v1/memory_stores/{sid}/memories/{mid}` — journals a
    /// `deleted` version; the chain is retained for audit.
    pub async fn delete_memory(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        raw_mem_id: &str,
        expected_content_sha256: Option<&str>,
    ) -> Result<Memory, ApiError> {
        let (sid, mid) = parse_mem_pair(raw_store_id, raw_mem_id)?;
        let mut record = self.store.get_memory(&ctx.workspace, &sid, &mid).await.map_err(map_store)?;
        if let Some(expected) = expected_content_sha256 {
            validate_sha256_hex("expected_content_sha256", expected)?;
            if record.head.content_sha256 != expected {
                return Err(precondition_failed());
            }
        }
        let now = Timestamp::now();
        let path = record.head.path.clone();
        let version = MemoryVersion::record(
            ctx.actor(),
            mid.clone(),
            sid.clone(),
            VersionOperation::Deleted,
            path,
            None,
            now,
        );
        record.chain.push(VersionRecord { version, blob: None });
        record.deleted = true;
        record.head.updated_at = now;
        let head = record.head.clone();
        self.store.replace_memory(&ctx.workspace, &sid, record).await.map_err(map_store)?;
        Ok(head)
    }
}

fn check_precondition(record: &MemoryRecord, pre: Option<&Precondition>) -> Result<(), ApiError> {
    if let Some(Precondition::ContentSha256 { content_sha256 }) = pre {
        validate_sha256_hex("precondition.content_sha256", content_sha256)?;
        if &record.head.content_sha256 != content_sha256 {
            return Err(precondition_failed());
        }
    }
    Ok(())
}

fn precondition_failed() -> ApiError {
    ApiError {
        status: 409,
        error_type: ErrorType::MemoryPreconditionFailed,
        message: "precondition failed: content_sha256 does not match the current content".into(),
        extra: serde_json::Map::new(),
    }
}

fn validate_path(path: &str) -> Result<(), ApiError> {
    if !path.starts_with('/') || path.len() < 2 {
        return Err(ApiError::invalid_request("path: must be an absolute path like \"/notes.md\""));
    }
    Ok(())
}
