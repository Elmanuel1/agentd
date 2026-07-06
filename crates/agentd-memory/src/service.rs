//! Memory business logic — wire-exact behaviors + the versioning extension.

use std::sync::Arc;

use agentd_core::hash::content_sha256;
use agentd_core::limits::{required, validate_bytes_max, validate_sha256_hex, values};
use agentd_core::metadata::MetadataPolicy;
use agentd_core::{ApiError, AuthContext, ErrorType, Pagination, Timestamp};

use crate::store::{MemoryPersistence, MemoryRecord, StoreError, VersionRecord, VersionSelector};
use crate::types::{
    CreateMemoryRequest, CreateStoreRequest, Memory, MemoryId, MemoryListItem, MemoryListPage,
    MemoryPrefix, MemoryPrefixTag, MemoryStoreEntity, MemoryStoreId, MemoryStoreTag, MemoryTag,
    MemoryVersion, MemoryVersionId, MemoryVersionTag, Precondition, UpdateMemoryRequest,
    VersionOperation,
};

/// List ordering — matches the live `order_by` values (default `created_at`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryOrder {
    /// Newest first.
    CreatedAt,
    /// Path ascending.
    Path,
}

impl MemoryOrder {
    fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        match raw {
            None | Some("created_at") => Ok(Self::CreatedAt),
            Some("path") => Ok(Self::Path),
            Some(_) => Err(ApiError::invalid_request(
                "order_by must be one of: created_at, path",
            )),
        }
    }
}

/// Normalize a `path_prefix` to a directory boundary: default `/`, always
/// trailing-slash terminated (live treats `/notes` == `/notes/`).
fn normalize_prefix(raw: Option<&str>) -> String {
    let p = raw.filter(|s| !s.is_empty()).unwrap_or("/");
    if p.ends_with('/') {
        p.to_owned()
    } else {
        format!("{p}/")
    }
}

fn item_path(item: &MemoryListItem) -> &str {
    match item {
        MemoryListItem::Memory(m) => &m.path,
        MemoryListItem::Prefix(p) => &p.path,
    }
}

/// Collapse memories whose path is deeper than `depth` (relative to `prefix`)
/// into `memory_prefix` directory entries. `depth` counts path segments below
/// the prefix — a memory with ≤ `depth` segments is a leaf, deeper ones fold
/// into the first-`depth`-segment directory.
fn collapse_depth(
    memories: Vec<Memory>,
    prefix: &str,
    depth: u32,
    order: MemoryOrder,
) -> Vec<MemoryListItem> {
    use std::collections::BTreeSet;
    let mut items: Vec<MemoryListItem> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for m in memories {
        let rel = m.path.strip_prefix(prefix).unwrap_or(m.path.as_str());
        let segs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        if (segs.len() as u32) <= depth {
            items.push(MemoryListItem::Memory(Box::new(m)));
        } else {
            let taken: Vec<&str> = segs.into_iter().take(depth as usize).collect();
            let dir = format!("{prefix}{}/", taken.join("/"));
            if seen.insert(dir.clone()) {
                items.push(MemoryListItem::Prefix(MemoryPrefix {
                    path: dir,
                    tag: MemoryPrefixTag::MemoryPrefix,
                }));
            }
        }
    }
    if matches!(order, MemoryOrder::Path) {
        items.sort_by(|a, b| item_path(a).cmp(item_path(b)));
    }
    items
}

/// Read view: `basic` omits content, `full` includes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Metadata only (list default).
    Basic,
    /// Include content.
    Full,
}

/// Memory service.
pub struct MemoriesService {
    store: Arc<dyn MemoryPersistence>,
}

impl std::fmt::Debug for MemoriesService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoriesService").finish_non_exhaustive()
    }
}

impl MemoriesService {
    /// Build with a persistence backend.
    pub fn new(store: Arc<dyn MemoryPersistence>) -> Self {
        Self { store }
    }

    // ── Stores ────────────────────────────────────────────────────────────

    /// `POST /v1/memory_stores`.
    pub async fn create_store(
        &self,
        ctx: &AuthContext,
        req: CreateStoreRequest,
    ) -> Result<MemoryStoreEntity, ApiError> {
        let name = required("name", req.name)?;
        req.metadata.validate(MetadataPolicy::capped())?;
        let now = Timestamp::now();
        let s = MemoryStoreEntity {
            archived_at: None,
            created_at: now,
            description: req.description.unwrap_or_default(),
            id: MemoryStoreId::generate(),
            metadata: req.metadata,
            name,
            tag: MemoryStoreTag::MemoryStore,
            updated_at: now,
        };
        self.store
            .insert_store(&ctx.workspace, s.clone())
            .await
            .map_err(map_store)?;
        Ok(s)
    }

    /// `GET /v1/memory_stores/{id}`.
    pub async fn get_store(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
    ) -> Result<MemoryStoreEntity, ApiError> {
        let id = parse_store_id(raw_id)?;
        self.store
            .get_store(&ctx.workspace, &id)
            .await
            .map_err(map_store)
    }

    /// `GET /v1/memory_stores`.
    pub async fn list_stores(
        &self,
        ctx: &AuthContext,
        page: &Pagination,
    ) -> Result<Vec<MemoryStoreEntity>, ApiError> {
        self.store
            .list_stores(&ctx.workspace, page)
            .await
            .map_err(map_store)
    }

    /// `POST /v1/memory_stores/{id}/archive`.
    pub async fn archive_store(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
    ) -> Result<MemoryStoreEntity, ApiError> {
        let id = parse_store_id(raw_id)?;
        let mut s = self
            .store
            .get_store(&ctx.workspace, &id)
            .await
            .map_err(map_store)?;
        if s.archived_at.is_none() {
            s.archived_at = Some(Timestamp::now());
            s.updated_at = Timestamp::now();
            self.store
                .replace_store(&ctx.workspace, s.clone())
                .await
                .map_err(map_store)?;
        }
        Ok(s)
    }

    /// `DELETE /v1/memory_stores/{id}` — live returned 200.
    pub async fn delete_store(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
    ) -> Result<MemoryStoreEntity, ApiError> {
        let id = parse_store_id(raw_id)?;
        self.store
            .delete_store(&ctx.workspace, &id)
            .await
            .map_err(map_store)
    }

    // ── Memories ──────────────────────────────────────────────────────────

    /// `POST /v1/memory_stores/{id}/memories` — 409 on occupied path.
    pub async fn create_memory(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        req: CreateMemoryRequest,
    ) -> Result<Memory, ApiError> {
        let store_id = parse_store_id(raw_store_id)?;
        let store = self
            .store
            .get_store(&ctx.workspace, &store_id)
            .await
            .map_err(map_store)?;
        if store.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request(
                "cannot write to an archived memory store",
            ));
        }
        let path = required("path", req.path)?;
        validate_path(&path)?;
        let content = required("content", req.content)?;
        validate_bytes_max("content", content.len(), values::MEMORY_CONTENT_MAX_BYTES)?;
        let now = Timestamp::now();
        let mem_id = MemoryId::generate();
        let ver_id = MemoryVersionId::generate();
        let sha = content_sha256(content.as_bytes());
        let head = Memory {
            content: None, // live: create response does NOT echo content
            content_sha256: sha.clone(),
            content_size_bytes: content.len() as u64,
            created_at: now,
            id: mem_id.clone(),
            memory_store_id: store_id.clone(),
            memory_version_id: ver_id.clone(),
            path: path.clone(),
            tag: MemoryTag::Memory,
            updated_at: now,
        };
        let version = MemoryVersion {
            content: None,
            content_sha256: Some(sha),
            content_size_bytes: Some(content.len() as u64),
            created_at: now,
            created_by: ctx.actor(),
            id: ver_id,
            memory_id: mem_id,
            memory_store_id: store_id.clone(),
            operation: VersionOperation::Created,
            path: Some(path),
            redacted_by: None,
            tag: MemoryVersionTag::MemoryVersion,
        };
        let record = MemoryRecord {
            head: head.clone(),
            deleted: false,
            chain: vec![VersionRecord {
                version,
                blob: Some(content),
            }],
        };
        self.store
            .insert_memory(&ctx.workspace, &store_id, record)
            .await
            .map_err(map_store)?;
        Ok(head)
    }

    /// `GET /v1/memory_stores/{sid}/memories/{mid}` — retrieve defaults to
    /// full view (content included).
    pub async fn get_memory(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        raw_mem_id: &str,
        view: View,
    ) -> Result<Memory, ApiError> {
        let (sid, mid) = parse_mem_pair(raw_store_id, raw_mem_id)?;
        let record = self
            .store
            .get_memory(&ctx.workspace, &sid, &mid)
            .await
            .map_err(map_store)?;
        Ok(project_head(&record, view))
    }

    /// `GET /v1/memory_stores/{id}/memories` — directory-style browse.
    ///
    /// `path_prefix` scopes the subtree (default `/`); `order_by` is
    /// `created_at` (newest first, default) or `path` (ascending); `depth`
    /// (relative to the prefix) collapses deeper paths into `memory_prefix`
    /// entries. Content is always omitted (basic view). Matches the live wire
    /// shape `{data, prefixes: []}`.
    pub async fn list_memories(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        path_prefix: Option<&str>,
        order_by: Option<&str>,
        depth: Option<u32>,
    ) -> Result<MemoryListPage, ApiError> {
        let sid = parse_store_id(raw_store_id)?;
        let order = MemoryOrder::parse(order_by)?;
        let prefix = normalize_prefix(path_prefix);
        let records = self
            .store
            .list_memories(&ctx.workspace, &sid, Some(&prefix))
            .await
            .map_err(map_store)?;

        let mut memories: Vec<Memory> =
            records.iter().map(|r| project_head(r, View::Basic)).collect();
        match order {
            MemoryOrder::Path => memories.sort_by(|a, b| a.path.cmp(&b.path)),
            MemoryOrder::CreatedAt => memories.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
        }

        let data = match depth {
            None => memories
                .into_iter()
                .map(|m| MemoryListItem::Memory(Box::new(m)))
                .collect(),
            Some(d) => collapse_depth(memories, &prefix, d, order),
        };
        Ok(MemoryListPage {
            data,
            prefixes: Vec::new(),
        })
    }

    /// Resolve a live memory by its path, with full content — used by the
    /// dream review gate to compare a candidate against current memory.
    pub async fn content_at_path(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        path: &str,
    ) -> Result<Option<Memory>, ApiError> {
        let sid = parse_store_id(raw_store_id)?;
        let Some(mid) = self
            .store
            .find_by_path(&ctx.workspace, &sid, path)
            .await
            .map_err(map_store)?
        else {
            return Ok(None);
        };
        let record = self
            .store
            .get_memory(&ctx.workspace, &sid, &mid)
            .await
            .map_err(map_store)?;
        Ok(Some(project_head(&record, View::Full)))
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
        let mut record = self
            .store
            .get_memory(&ctx.workspace, &sid, &mid)
            .await
            .map_err(map_store)?;
        check_precondition(&record, req.precondition.as_ref())?;
        let new_content = match req.content {
            Some(c) => {
                validate_bytes_max("content", c.len(), values::MEMORY_CONTENT_MAX_BYTES)?;
                c
            }
            None => record
                .resolve(&VersionSelector::Latest)
                .and_then(|v| v.blob.clone())
                .unwrap_or_default(),
        };
        let new_path = match req.path {
            Some(p) => {
                validate_path(&p)?;
                p
            }
            None => record.head.path.clone(),
        };
        let now = Timestamp::now();
        let ver_id = MemoryVersionId::generate();
        let sha = content_sha256(new_content.as_bytes());
        let version = MemoryVersion {
            content: None,
            content_sha256: Some(sha.clone()),
            content_size_bytes: Some(new_content.len() as u64),
            created_at: now,
            created_by: ctx.actor(),
            id: ver_id.clone(),
            memory_id: mid.clone(),
            memory_store_id: sid.clone(),
            operation: VersionOperation::Modified,
            path: Some(new_path.clone()),
            redacted_by: None,
            tag: MemoryVersionTag::MemoryVersion,
        };
        record.chain.push(VersionRecord {
            version,
            blob: Some(new_content),
        });
        // Advance the `latest` ref.
        record.head.memory_version_id = ver_id;
        record.head.content_sha256 = sha;
        record.head.content_size_bytes = record
            .chain
            .last()
            .and_then(|v| v.version.content_size_bytes)
            .unwrap_or_default();
        record.head.path = new_path;
        record.head.updated_at = now;
        let head = record.head.clone();
        self.store
            .replace_memory(&ctx.workspace, &sid, record)
            .await
            .map_err(map_store)?;
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
        let mut record = self
            .store
            .get_memory(&ctx.workspace, &sid, &mid)
            .await
            .map_err(map_store)?;
        if let Some(expected) = expected_content_sha256 {
            validate_sha256_hex("expected_content_sha256", expected)?;
            if record.head.content_sha256 != expected {
                return Err(precondition_failed());
            }
        }
        let now = Timestamp::now();
        record.chain.push(VersionRecord {
            version: MemoryVersion {
                content: None,
                content_sha256: None,
                content_size_bytes: None,
                created_at: now,
                created_by: ctx.actor(),
                id: MemoryVersionId::generate(),
                memory_id: mid.clone(),
                memory_store_id: sid.clone(),
                operation: VersionOperation::Deleted,
                path: Some(record.head.path.clone()),
                redacted_by: None,
                tag: MemoryVersionTag::MemoryVersion,
            },
            blob: None,
        });
        record.deleted = true;
        record.head.updated_at = now;
        let head = record.head.clone();
        self.store
            .replace_memory(&ctx.workspace, &sid, record)
            .await
            .map_err(map_store)?;
        Ok(head)
    }

    // ── Versions ──────────────────────────────────────────────────────────

    /// `GET /v1/memory_stores/{id}/memory_versions`.
    pub async fn list_versions(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        raw_memory_id: Option<&str>,
        view: View,
    ) -> Result<Vec<MemoryVersion>, ApiError> {
        let sid = parse_store_id(raw_store_id)?;
        let mid = match raw_memory_id {
            Some(raw) => Some(parse_mem_id(raw)?),
            None => None,
        };
        let versions = self
            .store
            .list_versions(&ctx.workspace, &sid, mid.as_ref())
            .await
            .map_err(map_store)?;
        Ok(versions.iter().map(|v| project_version(v, view)).collect())
    }

    /// `GET /v1/memory_stores/{sid}/memory_versions/{vid}` — full view.
    pub async fn get_version(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        raw_version_id: &str,
    ) -> Result<MemoryVersion, ApiError> {
        let sid = parse_store_id(raw_store_id)?;
        let vid = parse_version_id(raw_version_id)?;
        let (_, v) = self
            .store
            .get_version(&ctx.workspace, &sid, &vid)
            .await
            .map_err(map_store)?;
        Ok(project_version(&v, View::Full))
    }

    /// `POST /v1/memory_stores/{sid}/memory_versions/{vid}/redact` — scrub
    /// content while preserving actor + timestamps.
    pub async fn redact_version(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        raw_version_id: &str,
    ) -> Result<MemoryVersion, ApiError> {
        let sid = parse_store_id(raw_store_id)?;
        let vid = parse_version_id(raw_version_id)?;
        let (mem_id, mut v) = self
            .store
            .get_version(&ctx.workspace, &sid, &vid)
            .await
            .map_err(map_store)?;
        // Redact: clear content/sha/size/path, stamp redacted_by.
        v.blob = None;
        v.version.content_sha256 = None;
        v.version.content_size_bytes = None;
        v.version.path = None;
        v.version.redacted_by = Some(ctx.actor());
        // Persist by rewriting the record's chain entry.
        let mut record = self
            .store
            .get_memory(&ctx.workspace, &sid, &mem_id)
            .await
            .map_err(map_store)?;
        for entry in &mut record.chain {
            if entry.version.id == vid {
                *entry = v.clone();
            }
        }
        self.store
            .replace_memory(&ctx.workspace, &sid, record)
            .await
            .map_err(map_store)?;
        Ok(project_version(&v, View::Basic))
    }

    // ── Extension: resolve + rollback (the promote-don't-replace ops) ─────

    /// Resolve a memory to a concrete version (session-create snapshotting).
    pub async fn resolve(
        &self,
        ctx: &AuthContext,
        store_id: &MemoryStoreId,
        mem_id: &MemoryId,
        selector: &VersionSelector,
    ) -> Result<MemoryVersion, ApiError> {
        let record = self
            .store
            .get_memory(&ctx.workspace, store_id, mem_id)
            .await
            .map_err(map_store)?;
        record
            .resolve(selector)
            .map(|v| project_version(v, View::Full))
            .ok_or_else(|| ApiError::not_found("memory version not found"))
    }

    /// Repoint `latest` to an earlier version — O(1), non-destructive.
    pub async fn rollback(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        raw_mem_id: &str,
        raw_version_id: &str,
    ) -> Result<Memory, ApiError> {
        let (sid, mid) = parse_mem_pair(raw_store_id, raw_mem_id)?;
        let vid = parse_version_id(raw_version_id)?;
        let record = self
            .store
            .rollback(&ctx.workspace, &sid, &mid, &vid)
            .await
            .map_err(map_store)?;
        Ok(record.head)
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

fn project_head(record: &MemoryRecord, view: View) -> Memory {
    let mut head = record.head.clone();
    head.content = match view {
        View::Full => record
            .resolve(&VersionSelector::Latest)
            .and_then(|v| v.blob.clone()),
        View::Basic => None,
    };
    head
}

fn project_version(v: &VersionRecord, view: View) -> MemoryVersion {
    let mut out = v.version.clone();
    out.content = match view {
        View::Full => v.blob.clone(),
        View::Basic => None,
    };
    out
}

fn check_precondition(
    record: &MemoryRecord,
    pre: Option<&Precondition>,
) -> Result<(), ApiError> {
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
        return Err(ApiError::invalid_request(
            "path: must be an absolute path like \"/notes.md\"",
        ));
    }
    Ok(())
}

fn parse_store_id(raw: &str) -> Result<MemoryStoreId, ApiError> {
    MemoryStoreId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn parse_mem_id(raw: &str) -> Result<MemoryId, ApiError> {
    MemoryId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn parse_version_id(raw: &str) -> Result<MemoryVersionId, ApiError> {
    MemoryVersionId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn parse_mem_pair(s: &str, m: &str) -> Result<(MemoryStoreId, MemoryId), ApiError> {
    Ok((parse_store_id(s)?, parse_mem_id(m)?))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::StoreNotFound => ApiError::not_found("memory store not found"),
        StoreError::MemoryNotFound => ApiError::not_found("memory not found"),
        StoreError::VersionNotFound => ApiError::not_found("memory version not found"),
        StoreError::PathConflict { path, memory_id } => {
            ApiError::memory_path_conflict(&path, memory_id.as_str())
        }
        StoreError::Backend(m) => ApiError::internal(m),
    }
}
