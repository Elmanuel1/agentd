use shared::hash::content_sha256;
use shared::limits::{required, validate_bytes_max, validate_sha256_hex, values};
use shared::{ApiError, AuthContext, Timestamp};

use super::{map_store, parse_mem_pair, parse_store_id, MemoriesService};
use crate::store::{MemoryRecord, StoreError, VersionRecord, VersionSelector};
use crate::types::{
    CreateMemoryRequest, Memory, MemoryId, MemoryStoreId, MemoryTag, MemoryVersion, Precondition,
    UpdateMemoryRequest, VersionOperation,
};

impl MemoriesService {
    /// An archived store is read-only.
    async fn require_writable(&self, ctx: &AuthContext, id: &MemoryStoreId) -> Result<(), ApiError> {
        let store = self.store.get_store(&ctx.workspace, id).await.map_err(map_store)?;
        if store.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request("cannot write to an archived memory store"));
        }
        Ok(())
    }

    /// `POST /v1/memory_stores/{id}/memories` — 409 on occupied path.
    pub async fn create_memory(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        req: CreateMemoryRequest,
    ) -> Result<Memory, ApiError> {
        let store_id = parse_store_id(raw_store_id)?;
        self.require_writable(ctx, &store_id).await?;
        let path = required("path", req.path)?;
        Memory::validate_path(&path)?;
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
        self.require_writable(ctx, &sid).await?;
        if let Some(c) = &req.content {
            validate_bytes_max("content", c.len(), values::MEMORY_CONTENT_MAX_BYTES)?;
        }
        if let Some(p) = &req.path {
            Memory::validate_path(p)?;
        }
        for _ in 0..HEAD_ATTEMPTS {
            if let Some(head) = self.try_update(ctx, &sid, &mid, &req).await? {
                return Ok(head);
            }
        }
        Err(map_store(StoreError::HeadMoved))
    }

    /// One read-and-append; `None` when another writer moved the head first,
    /// so the caller re-reads and applies the update to the new head (a
    /// precondition is then checked against that head).
    async fn try_update(
        &self,
        ctx: &AuthContext,
        sid: &MemoryStoreId,
        mid: &MemoryId,
        req: &UpdateMemoryRequest,
    ) -> Result<Option<Memory>, ApiError> {
        let record = self.store.get_memory(&ctx.workspace, sid, mid).await.map_err(map_store)?;
        let new_content = match &req.content {
            Some(c) => c.clone(),
            None => record.resolve(&VersionSelector::Latest).and_then(|v| v.blob.clone()).unwrap_or_default(),
        };
        let new_path = req.path.clone().unwrap_or_else(|| record.head.path.clone());
        // Live: an update that changes nothing writes no version, and that
        // holds even when its precondition names other content.
        if content_sha256(new_content.as_bytes()) == record.head.content_sha256
            && new_path == record.head.path
        {
            return Ok(Some(record.head));
        }
        if let Some(pre) = &req.precondition {
            pre.check(&record.head)?;
        }
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
        let mut head = record.head;
        let expected = std::mem::replace(&mut head.memory_version_id, version.id.clone());
        head.content_sha256 = version.content_sha256.clone().unwrap_or_default();
        head.content_size_bytes = new_content.len() as u64;
        head.path = new_path;
        head.updated_at = now;
        let version = VersionRecord { version, blob: Some(new_content) };
        let appended =
            self.store.append_version(&ctx.workspace, sid, &head, false, &expected, &version).await;
        moved_or(appended, head)
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
        self.require_writable(ctx, &sid).await?;
        let pre = expected_content_sha256
            .map(|sha| {
                validate_sha256_hex("expected_content_sha256", sha)?;
                Ok::<_, ApiError>(Precondition::ContentSha256 { content_sha256: sha.to_owned() })
            })
            .transpose()?;
        for _ in 0..HEAD_ATTEMPTS {
            let record = self.store.get_memory(&ctx.workspace, &sid, &mid).await.map_err(map_store)?;
            if let Some(pre) = &pre {
                pre.check(&record.head)?;
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
            let mut head = record.head;
            head.updated_at = now;
            let expected = head.memory_version_id.clone();
            let version = VersionRecord { version, blob: None };
            let appended =
                self.store.append_version(&ctx.workspace, &sid, &head, true, &expected, &version).await;
            if let Some(head) = moved_or(appended, head)? {
                return Ok(head);
            }
        }
        Err(map_store(StoreError::HeadMoved))
    }
}

/// Reads before a write that keeps losing the head to other writers.
const HEAD_ATTEMPTS: usize = 3;

fn moved_or(appended: Result<(), StoreError>, head: Memory) -> Result<Option<Memory>, ApiError> {
    match appended {
        Ok(()) => Ok(Some(head)),
        Err(StoreError::HeadMoved) => Ok(None),
        Err(e) => Err(map_store(e)),
    }
}
