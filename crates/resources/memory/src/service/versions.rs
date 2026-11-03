use shared::{ApiError, AuthContext, Pagination};

use super::{map_store, parse_mem_id, parse_mem_pair, parse_store_id, MemoriesService, View};
use crate::store::{StoreError, VersionFilter, VersionRecord, VersionSelector};
use crate::types::{Memory, MemoryId, MemoryStoreId, MemoryVersion, MemoryVersionId};

impl MemoriesService {
    /// `GET /v1/memory_stores/{id}/memory_versions`.
    pub async fn list_versions(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        filter: &VersionFilter,
        view: View,
        page: &Pagination,
    ) -> Result<Vec<MemoryVersion>, ApiError> {
        let sid = parse_store_id(raw_store_id)?;
        if let Some(raw) = &filter.memory_id {
            parse_mem_id(raw)?;
        }
        let mut versions =
            self.store.list_versions(&ctx.workspace, &sid, filter, page).await.map_err(map_store)?;
        if view == View::Full {
            // The list read carries no content; a full page reads each
            // version's content concurrently.
            let reads = versions.iter_mut().take(page.limit).map(|v| async {
                v.blob = self.store.get_version(&ctx.workspace, &sid, &v.version.id).await?.1.blob;
                Ok::<_, crate::store::StoreError>(())
            });
            futures::future::try_join_all(reads).await.map_err(map_store)?;
        }
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
        let (_, v) = self.store.get_version(&ctx.workspace, &sid, &vid).await.map_err(map_store)?;
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
        let (mem_id, mut v) = self.store.get_version(&ctx.workspace, &sid, &vid).await.map_err(map_store)?;
        // A version whose memory is not found belongs to a deleted memory.
        let live_head = match self.store.get_memory(&ctx.workspace, &sid, &mem_id).await {
            Ok(record) => record.head.memory_version_id == vid,
            Err(StoreError::MemoryNotFound) => false,
            Err(e) => return Err(map_store(e)),
        };
        // Live: the current head of a live memory cannot be redacted.
        if live_head {
            return Err(ApiError::conflict_invalid_request(
                "cannot redact the current version of a live memory; update or delete the memory first",
            ));
        }
        // Redact: clear content/sha/size/path, stamp redacted_by.
        v.blob = None;
        v.version.content_sha256 = None;
        v.version.content_size_bytes = None;
        v.version.path = None;
        v.version.redacted_by = Some(ctx.actor());
        v.version.redacted_at = Some(shared::Timestamp::now());
        self.store.replace_version(&ctx.workspace, &sid, &mem_id, &v).await.map_err(map_store)?;
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
        let record = self.store.get_memory(&ctx.workspace, store_id, mem_id).await.map_err(map_store)?;
        let version = match selector {
            VersionSelector::Latest => record.resolve(selector).cloned(),
            VersionSelector::Pinned(id) => {
                let (owner, v) =
                    self.store.get_version(&ctx.workspace, store_id, id).await.map_err(map_store)?;
                (&owner == mem_id).then_some(v)
            }
        };
        version
            .map(|v| project_version(&v, View::Full))
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
        let record = self.store.rollback(&ctx.workspace, &sid, &mid, &vid).await.map_err(map_store)?;
        Ok(record.head)
    }
}

fn project_version(v: &VersionRecord, view: View) -> MemoryVersion {
    let mut out = v.version.clone();
    out.content = match view {
        View::Full => v.blob.clone(),
        View::Basic => None,
    };
    out
}

fn parse_version_id(raw: &str) -> Result<MemoryVersionId, ApiError> {
    MemoryVersionId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}
