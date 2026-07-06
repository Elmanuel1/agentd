//! `MemoryPersistence` on Postgres via Diesel — the git-object model:
//! `memory_stores` → `memories` (head) → `memory_versions` (append-only chain).
//! Version CONTENT is a blob in object storage, keyed by version id.

use std::sync::Arc;

use agentd_blobs::BlobStore;
use agentd_core::{Pagination, WorkspaceId};
use agentd_memory::store::{MemoryPersistence, MemoryRecord, StoreError, VersionRecord};
use agentd_memory::types::{
    Memory, MemoryId, MemoryStoreEntity, MemoryStoreId, MemoryVersion, MemoryVersionId,
};
use async_trait::async_trait;
use bytes::Bytes;
use diesel::upsert::excluded;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, TextExpressionMethods};
use diesel_async::RunQueryDsl;

use crate::schema::{memories, memory_stores, memory_versions};
use crate::{blob_be, diesel_be, parse_doc, pool_be, to_doc, PgBackend};

/// Postgres + object-storage backed memory store (Diesel).
#[derive(Clone)]
pub struct PgMemoryStore {
    backend: PgBackend,
    blobs: Arc<dyn BlobStore>,
}

impl std::fmt::Debug for PgMemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgMemoryStore").finish_non_exhaustive()
    }
}

impl PgMemoryStore {
    /// Wrap a backend + blob store.
    pub fn new(backend: PgBackend, blobs: Arc<dyn BlobStore>) -> Self {
        Self { backend, blobs }
    }

    fn blob_key(ws: &WorkspaceId, store: &MemoryStoreId, version: &MemoryVersionId) -> String {
        format!("memory/{}/{}/{}", ws.as_str(), store.as_str(), version.as_str())
    }

    async fn require_store(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
    ) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let exists: Option<String> = memory_stores::table
            .filter(memory_stores::workspace_id.eq(ws.as_str()))
            .filter(memory_stores::store_id.eq(store.as_str()))
            .select(memory_stores::store_id)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        exists.map(|_| ()).ok_or(StoreError::StoreNotFound)
    }

    async fn chain_full(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        memory_id: &MemoryId,
    ) -> Result<Vec<VersionRecord>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<(serde_json::Value, bool)> = memory_versions::table
            .filter(memory_versions::workspace_id.eq(ws.as_str()))
            .filter(memory_versions::store_id.eq(store.as_str()))
            .filter(memory_versions::memory_id.eq(memory_id.as_str()))
            .order(memory_versions::seq.asc())
            .select((memory_versions::doc, memory_versions::has_blob))
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        // Fetch the chain's content blobs concurrently — they're independent
        // object-store reads, so a sequential loop would add latency linearly
        // on the hot `get_memory` path.
        let fetches = rows.into_iter().map(|(doc, has_blob)| async move {
            let version: MemoryVersion = parse_doc(doc)?;
            let blob = if has_blob {
                let bytes = self
                    .blobs
                    .get(&Self::blob_key(ws, store, &version.id))
                    .await
                    .map_err(blob_be)?;
                Some(String::from_utf8_lossy(&bytes).into_owned())
            } else {
                None
            };
            Ok(VersionRecord { version, blob })
        });
        futures::future::try_join_all(fetches).await
    }

    async fn upsert_version(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        memory_id: &MemoryId,
        v: &VersionRecord,
    ) -> Result<(), StoreError> {
        let key = Self::blob_key(ws, store, &v.version.id);
        match &v.blob {
            Some(content) => self
                .blobs
                .put(&key, Bytes::from(content.clone().into_bytes()))
                .await
                .map_err(blob_be)?,
            None => match self.blobs.delete(&key).await {
                Ok(()) | Err(agentd_blobs::BlobError::NotFound(_)) => {}
                Err(e) => return Err(blob_be(e)),
            },
        }
        let mut meta = v.version.clone();
        meta.content = None;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(memory_versions::table)
            .values((
                memory_versions::workspace_id.eq(ws.as_str()),
                memory_versions::store_id.eq(store.as_str()),
                memory_versions::memory_id.eq(memory_id.as_str()),
                memory_versions::version_id.eq(v.version.id.as_str()),
                memory_versions::has_blob.eq(v.blob.is_some()),
                memory_versions::created_at.eq(v.version.created_at.as_datetime()),
                memory_versions::doc.eq(to_doc(&meta)?),
            ))
            .on_conflict((
                memory_versions::workspace_id,
                memory_versions::store_id,
                memory_versions::version_id,
            ))
            .do_update()
            .set((
                memory_versions::has_blob.eq(excluded(memory_versions::has_blob)),
                memory_versions::doc.eq(excluded(memory_versions::doc)),
            ))
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn upsert_head(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: &MemoryRecord,
    ) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(memories::table)
            .values((
                memories::workspace_id.eq(ws.as_str()),
                memories::store_id.eq(store.as_str()),
                memories::memory_id.eq(record.head.id.as_str()),
                memories::path.eq(&record.head.path),
                memories::head_version_id.eq(record.head.memory_version_id.as_str()),
                memories::deleted.eq(record.deleted),
                memories::created_at.eq(record.head.created_at.as_datetime()),
                memories::doc.eq(to_doc(&record.head)?),
            ))
            .on_conflict((
                memories::workspace_id,
                memories::store_id,
                memories::memory_id,
            ))
            .do_update()
            .set((
                memories::path.eq(excluded(memories::path)),
                memories::head_version_id.eq(excluded(memories::head_version_id)),
                memories::deleted.eq(excluded(memories::deleted)),
                memories::doc.eq(excluded(memories::doc)),
            ))
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }
}

#[async_trait]
impl MemoryPersistence for PgMemoryStore {
    async fn insert_store(&self, ws: &WorkspaceId, s: MemoryStoreEntity) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(memory_stores::table)
            .values((
                memory_stores::workspace_id.eq(ws.as_str()),
                memory_stores::store_id.eq(s.id.as_str()),
                memory_stores::name.eq(&s.name),
                memory_stores::created_at.eq(s.created_at.as_datetime()),
                memory_stores::doc.eq(to_doc(&s)?),
            ))
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get_store(
        &self,
        ws: &WorkspaceId,
        id: &MemoryStoreId,
    ) -> Result<MemoryStoreEntity, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = memory_stores::table
            .filter(memory_stores::workspace_id.eq(ws.as_str()))
            .filter(memory_stores::store_id.eq(id.as_str()))
            .select(memory_stores::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::StoreNotFound)?)
    }

    async fn replace_store(&self, ws: &WorkspaceId, s: MemoryStoreEntity) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let affected = diesel::update(
            memory_stores::table
                .filter(memory_stores::workspace_id.eq(ws.as_str()))
                .filter(memory_stores::store_id.eq(s.id.as_str())),
        )
        .set((memory_stores::name.eq(&s.name), memory_stores::doc.eq(to_doc(&s)?)))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::StoreNotFound);
        }
        Ok(())
    }

    async fn delete_store(
        &self,
        ws: &WorkspaceId,
        id: &MemoryStoreId,
    ) -> Result<MemoryStoreEntity, StoreError> {
        let store = self.get_store(ws, id).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let version_ids: Vec<String> = memory_versions::table
            .filter(memory_versions::workspace_id.eq(ws.as_str()))
            .filter(memory_versions::store_id.eq(id.as_str()))
            .filter(memory_versions::has_blob.eq(true))
            .select(memory_versions::version_id)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        diesel::delete(
            memory_versions::table
                .filter(memory_versions::workspace_id.eq(ws.as_str()))
                .filter(memory_versions::store_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        diesel::delete(
            memories::table
                .filter(memories::workspace_id.eq(ws.as_str()))
                .filter(memories::store_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        diesel::delete(
            memory_stores::table
                .filter(memory_stores::workspace_id.eq(ws.as_str()))
                .filter(memory_stores::store_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        for vid in version_ids {
            let key = format!("memory/{}/{}/{}", ws.as_str(), id.as_str(), vid);
            match self.blobs.delete(&key).await {
                Ok(()) | Err(agentd_blobs::BlobError::NotFound(_)) => {}
                Err(e) => return Err(blob_be(e)),
            }
        }
        Ok(store)
    }

    async fn list_stores(
        &self,
        ws: &WorkspaceId,
        page: &Pagination,
    ) -> Result<Vec<MemoryStoreEntity>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = memory_stores::table
            .filter(memory_stores::workspace_id.eq(ws.as_str()))
            .order(memory_stores::created_at.desc())
            .select(memory_stores::doc)
            .limit(page.fetch_limit())
            .offset(page.sql_offset())
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }

    async fn insert_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: MemoryRecord,
    ) -> Result<(), StoreError> {
        self.require_store(ws, store).await?;
        if let Some(occupied) = self.find_by_path(ws, store, &record.head.path).await? {
            return Err(StoreError::PathConflict {
                path: record.head.path.clone(),
                memory_id: occupied,
            });
        }
        self.upsert_head(ws, store, &record).await?;
        for v in &record.chain {
            self.upsert_version(ws, store, &record.head.id, v).await?;
        }
        Ok(())
    }

    async fn get_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryId,
    ) -> Result<MemoryRecord, StoreError> {
        self.require_store(ws, store).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let row: Option<(serde_json::Value, bool)> = memories::table
            .filter(memories::workspace_id.eq(ws.as_str()))
            .filter(memories::store_id.eq(store.as_str()))
            .filter(memories::memory_id.eq(id.as_str()))
            .select((memories::doc, memories::deleted))
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        let (doc, deleted) = row.ok_or(StoreError::MemoryNotFound)?;
        if deleted {
            return Err(StoreError::MemoryNotFound);
        }
        let head: Memory = parse_doc(doc)?;
        let chain = self.chain_full(ws, store, id).await?;
        Ok(MemoryRecord {
            head,
            deleted,
            chain,
        })
    }

    async fn replace_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: MemoryRecord,
    ) -> Result<(), StoreError> {
        self.require_store(ws, store).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let exists: Option<String> = memories::table
            .filter(memories::workspace_id.eq(ws.as_str()))
            .filter(memories::store_id.eq(store.as_str()))
            .filter(memories::memory_id.eq(record.head.id.as_str()))
            .select(memories::memory_id)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        if exists.is_none() {
            return Err(StoreError::MemoryNotFound);
        }
        if !record.deleted {
            if let Some(other) = self.find_by_path(ws, store, &record.head.path).await? {
                if other != record.head.id {
                    return Err(StoreError::PathConflict {
                        path: record.head.path.clone(),
                        memory_id: other,
                    });
                }
            }
        }
        self.upsert_head(ws, store, &record).await?;
        for v in &record.chain {
            self.upsert_version(ws, store, &record.head.id, v).await?;
        }
        Ok(())
    }

    async fn list_memories(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        path_prefix: Option<&str>,
    ) -> Result<Vec<MemoryRecord>, StoreError> {
        self.require_store(ws, store).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let mut q = memories::table
            .filter(memories::workspace_id.eq(ws.as_str()))
            .filter(memories::store_id.eq(store.as_str()))
            .filter(memories::deleted.eq(false))
            .into_boxed();
        if let Some(p) = path_prefix {
            // Anchored prefix `LIKE 'p%'` — index-backed by the
            // `text_pattern_ops` index (V3). Escape the caller's `%`/`_`/`\` so
            // a path containing them can't act as a wildcard (ESCAPE '\').
            let escaped = p.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            q = q.filter(memories::path.like(format!("{escaped}%")));
        }
        let heads: Vec<serde_json::Value> = q
            .order(memories::path.asc())
            .select(memories::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        // The listing projects the head only (basic view), so skip the
        // per-memory version-chain fetch entirely — no N+1.
        heads
            .into_iter()
            .map(|doc| {
                Ok(MemoryRecord {
                    head: parse_doc(doc)?,
                    deleted: false,
                    chain: Vec::new(),
                })
            })
            .collect()
    }

    async fn find_by_path(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        path: &str,
    ) -> Result<Option<MemoryId>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = memories::table
            .filter(memories::workspace_id.eq(ws.as_str()))
            .filter(memories::store_id.eq(store.as_str()))
            .filter(memories::path.eq(path))
            .filter(memories::deleted.eq(false))
            .select(memories::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        match doc {
            Some(d) => {
                let m: Memory = parse_doc(d)?;
                Ok(Some(m.id))
            }
            None => Ok(None),
        }
    }

    async fn list_versions(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        memory_id: Option<&MemoryId>,
    ) -> Result<Vec<VersionRecord>, StoreError> {
        self.require_store(ws, store).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let mut q = memory_versions::table
            .filter(memory_versions::workspace_id.eq(ws.as_str()))
            .filter(memory_versions::store_id.eq(store.as_str()))
            .into_boxed();
        if let Some(mid) = memory_id {
            q = q.filter(memory_versions::memory_id.eq(mid.as_str().to_owned()));
        }
        let docs: Vec<serde_json::Value> = q
            .order(memory_versions::seq.desc())
            .select(memory_versions::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter()
            .map(|d| {
                Ok(VersionRecord {
                    version: parse_doc(d)?,
                    blob: None,
                })
            })
            .collect()
    }

    async fn get_version(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryVersionId,
    ) -> Result<(MemoryId, VersionRecord), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let row: Option<(serde_json::Value, bool)> = memory_versions::table
            .filter(memory_versions::workspace_id.eq(ws.as_str()))
            .filter(memory_versions::store_id.eq(store.as_str()))
            .filter(memory_versions::version_id.eq(id.as_str()))
            .select((memory_versions::doc, memory_versions::has_blob))
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        let (doc, has_blob) = row.ok_or(StoreError::VersionNotFound)?;
        let version: MemoryVersion = parse_doc(doc)?;
        let blob = if has_blob {
            let bytes = self
                .blobs
                .get(&Self::blob_key(ws, store, id))
                .await
                .map_err(blob_be)?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        } else {
            None
        };
        let memory_id = version.memory_id.clone();
        Ok((memory_id, VersionRecord { version, blob }))
    }

    async fn rollback(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryId,
        to: &MemoryVersionId,
    ) -> Result<MemoryRecord, StoreError> {
        let mut record = self.get_memory(ws, store, id).await?;
        let target = record
            .chain
            .iter()
            .find(|v| &v.version.id == to)
            .cloned()
            .ok_or(StoreError::VersionNotFound)?;
        record.head.memory_version_id = target.version.id.clone();
        if let (Some(sha), Some(size), Some(path)) = (
            target.version.content_sha256.clone(),
            target.version.content_size_bytes,
            target.version.path.clone(),
        ) {
            record.head.content_sha256 = sha;
            record.head.content_size_bytes = size;
            record.head.path = path;
        }
        self.upsert_head(ws, store, &record).await?;
        Ok(record)
    }
}
