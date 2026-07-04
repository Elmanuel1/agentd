//! `MemoryPersistence` on Postgres — the git-object model: `memory_stores` →
//! `memories` (head pointer) → `memory_versions` (append-only chain). Version
//! CONTENT is a blob in object storage, keyed by version id; only metadata and
//! the movable `latest` ref live in the DB.

use std::sync::Arc;

use agentd_blobs::BlobStore;
use agentd_core::WorkspaceId;
use agentd_memory::store::{
    MemoryPersistence, MemoryRecord, StoreError, VersionRecord,
};
use agentd_memory::types::{
    Memory, MemoryId, MemoryStoreEntity, MemoryStoreId, MemoryVersion, MemoryVersionId,
};
use async_trait::async_trait;
use bytes::Bytes;
use sqlx::types::Json;

use crate::{sqlx_msg, PgBackend};

/// Postgres + object-storage backed memory store.
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

fn be(e: sqlx::Error) -> StoreError {
    StoreError::Backend(sqlx_msg(e))
}

fn blob_be(e: agentd_blobs::BlobError) -> StoreError {
    StoreError::Backend(e.to_string())
}

impl PgMemoryStore {
    /// Wrap a pool + blob store.
    pub fn new(backend: PgBackend, blobs: Arc<dyn BlobStore>) -> Self {
        Self { backend, blobs }
    }

    fn blob_key(ws: &WorkspaceId, store: &MemoryStoreId, version: &MemoryVersionId) -> String {
        format!("memory/{}/{}/{}", ws.0, store.as_str(), version.as_str())
    }

    async fn require_store(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
    ) -> Result<(), StoreError> {
        let exists: Option<(i32,)> = sqlx::query_as(
            "select 1 from memory_stores where workspace_id = $1 and store_id = $2",
        )
        .bind(&ws.0)
        .bind(store.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        exists.map(|_| ()).ok_or(StoreError::StoreNotFound)
    }

    /// Chain metadata (content unset — `blob = None`), oldest first.
    async fn chain_meta(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        memory_id: &MemoryId,
    ) -> Result<Vec<VersionRecord>, StoreError> {
        let rows: Vec<(Json<MemoryVersion>,)> = sqlx::query_as(
            "select doc from memory_versions
             where workspace_id = $1 and store_id = $2 and memory_id = $3
             order by seq asc",
        )
        .bind(&ws.0)
        .bind(store.as_str())
        .bind(memory_id.as_str())
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows
            .into_iter()
            .map(|(d,)| VersionRecord {
                version: d.0,
                blob: None,
            })
            .collect())
    }

    /// Chain with content blobs fetched from object storage (full view).
    async fn chain_full(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        memory_id: &MemoryId,
    ) -> Result<Vec<VersionRecord>, StoreError> {
        let rows: Vec<(Json<MemoryVersion>, bool)> = sqlx::query_as(
            "select doc, has_blob from memory_versions
             where workspace_id = $1 and store_id = $2 and memory_id = $3
             order by seq asc",
        )
        .bind(&ws.0)
        .bind(store.as_str())
        .bind(memory_id.as_str())
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        let mut out = Vec::with_capacity(rows.len());
        for (doc, has_blob) in rows {
            let blob = if has_blob {
                let bytes = self
                    .blobs
                    .get(&Self::blob_key(ws, store, &doc.0.id))
                    .await
                    .map_err(blob_be)?;
                Some(String::from_utf8_lossy(&bytes).into_owned())
            } else {
                None
            };
            out.push(VersionRecord {
                version: doc.0,
                blob,
            });
        }
        Ok(out)
    }

    /// Persist one chain version: content → S3 (or purge on redact), metadata
    /// row upserted (seq preserved on conflict).
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
        sqlx::query(
            "insert into memory_versions
               (workspace_id, store_id, memory_id, version_id, has_blob, created_at, doc)
             values ($1, $2, $3, $4, $5, $6, $7)
             on conflict (workspace_id, store_id, version_id)
             do update set has_blob = excluded.has_blob, doc = excluded.doc",
        )
        .bind(&ws.0)
        .bind(store.as_str())
        .bind(memory_id.as_str())
        .bind(v.version.id.as_str())
        .bind(v.blob.is_some())
        .bind(v.version.created_at.as_datetime())
        .bind(Json(&meta))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn upsert_head(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: &MemoryRecord,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "insert into memories
               (workspace_id, store_id, memory_id, path, head_version_id, deleted, created_at, doc)
             values ($1, $2, $3, $4, $5, $6, $7, $8)
             on conflict (workspace_id, store_id, memory_id)
             do update set path = excluded.path, head_version_id = excluded.head_version_id,
                           deleted = excluded.deleted, doc = excluded.doc",
        )
        .bind(&ws.0)
        .bind(store.as_str())
        .bind(record.head.id.as_str())
        .bind(&record.head.path)
        .bind(record.head.memory_version_id.as_str())
        .bind(record.deleted)
        .bind(record.head.created_at.as_datetime())
        .bind(Json(&record.head))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }
}

#[async_trait]
impl MemoryPersistence for PgMemoryStore {
    async fn insert_store(&self, ws: &WorkspaceId, s: MemoryStoreEntity) -> Result<(), StoreError> {
        sqlx::query(
            "insert into memory_stores (workspace_id, store_id, name, created_at, doc)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(&ws.0)
        .bind(s.id.as_str())
        .bind(&s.name)
        .bind(s.created_at.as_datetime())
        .bind(Json(&s))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn get_store(
        &self,
        ws: &WorkspaceId,
        id: &MemoryStoreId,
    ) -> Result<MemoryStoreEntity, StoreError> {
        let row: Option<(Json<MemoryStoreEntity>,)> = sqlx::query_as(
            "select doc from memory_stores where workspace_id = $1 and store_id = $2",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        row.map(|(d,)| d.0).ok_or(StoreError::StoreNotFound)
    }

    async fn replace_store(&self, ws: &WorkspaceId, s: MemoryStoreEntity) -> Result<(), StoreError> {
        let affected = sqlx::query(
            "update memory_stores set name = $3, doc = $4
             where workspace_id = $1 and store_id = $2",
        )
        .bind(&ws.0)
        .bind(s.id.as_str())
        .bind(&s.name)
        .bind(Json(&s))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?
        .rows_affected();
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
        // Purge content blobs for every version in the store first.
        let versions: Vec<(String,)> = sqlx::query_as(
            "select version_id from memory_versions where workspace_id = $1 and store_id = $2
             and has_blob = true",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        let row: Option<(Json<MemoryStoreEntity>,)> = sqlx::query_as(
            "delete from memory_stores where workspace_id = $1 and store_id = $2 returning doc",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        let store = row.map(|(d,)| d.0).ok_or(StoreError::StoreNotFound)?;
        sqlx::query("delete from memory_versions where workspace_id = $1 and store_id = $2")
            .bind(&ws.0)
            .bind(id.as_str())
            .execute(&self.backend.pool)
            .await
            .map_err(be)?;
        sqlx::query("delete from memories where workspace_id = $1 and store_id = $2")
            .bind(&ws.0)
            .bind(id.as_str())
            .execute(&self.backend.pool)
            .await
            .map_err(be)?;
        for (vid,) in versions {
            let key = format!("memory/{}/{}/{}", ws.0, id.as_str(), vid);
            match self.blobs.delete(&key).await {
                Ok(()) | Err(agentd_blobs::BlobError::NotFound(_)) => {}
                Err(e) => return Err(blob_be(e)),
            }
        }
        Ok(store)
    }

    async fn list_stores(&self, ws: &WorkspaceId) -> Result<Vec<MemoryStoreEntity>, StoreError> {
        let rows: Vec<(Json<MemoryStoreEntity>,)> = sqlx::query_as(
            "select doc from memory_stores where workspace_id = $1 order by created_at desc",
        )
        .bind(&ws.0)
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
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
        let row: Option<(Json<Memory>, bool)> = sqlx::query_as(
            "select doc, deleted from memories
             where workspace_id = $1 and store_id = $2 and memory_id = $3",
        )
        .bind(&ws.0)
        .bind(store.as_str())
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        let (head, deleted) = row.ok_or(StoreError::MemoryNotFound)?;
        if deleted {
            return Err(StoreError::MemoryNotFound);
        }
        let chain = self.chain_full(ws, store, id).await?;
        Ok(MemoryRecord {
            head: head.0,
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
        let exists: Option<(i32,)> = sqlx::query_as(
            "select 1 from memories where workspace_id = $1 and store_id = $2 and memory_id = $3",
        )
        .bind(&ws.0)
        .bind(store.as_str())
        .bind(record.head.id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
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
        let heads: Vec<(Json<Memory>,)> = sqlx::query_as(
            "select doc from memories
             where workspace_id = $1 and store_id = $2 and deleted = false
               and ($3::text is null or path like $3 || '%')
             order by path",
        )
        .bind(&ws.0)
        .bind(store.as_str())
        .bind(path_prefix)
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        let mut out = Vec::with_capacity(heads.len());
        for (head,) in heads {
            let chain = self.chain_meta(ws, store, &head.0.id).await?;
            out.push(MemoryRecord {
                head: head.0,
                deleted: false,
                chain,
            });
        }
        Ok(out)
    }

    async fn find_by_path(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        path: &str,
    ) -> Result<Option<MemoryId>, StoreError> {
        let row: Option<(Json<Memory>,)> = sqlx::query_as(
            "select doc from memories
             where workspace_id = $1 and store_id = $2 and path = $3 and deleted = false",
        )
        .bind(&ws.0)
        .bind(store.as_str())
        .bind(path)
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(row.map(|(d,)| d.0.id))
    }

    async fn list_versions(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        memory_id: Option<&MemoryId>,
    ) -> Result<Vec<VersionRecord>, StoreError> {
        self.require_store(ws, store).await?;
        let rows: Vec<(Json<MemoryVersion>,)> = sqlx::query_as(
            "select doc from memory_versions
             where workspace_id = $1 and store_id = $2
               and ($3::text is null or memory_id = $3)
             order by seq desc",
        )
        .bind(&ws.0)
        .bind(store.as_str())
        .bind(memory_id.map(MemoryId::as_str))
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows
            .into_iter()
            .map(|(d,)| VersionRecord {
                version: d.0,
                blob: None,
            })
            .collect())
    }

    async fn get_version(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryVersionId,
    ) -> Result<(MemoryId, VersionRecord), StoreError> {
        let row: Option<(Json<MemoryVersion>, bool)> = sqlx::query_as(
            "select doc, has_blob from memory_versions
             where workspace_id = $1 and store_id = $2 and version_id = $3",
        )
        .bind(&ws.0)
        .bind(store.as_str())
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        let (doc, has_blob) = row.ok_or(StoreError::VersionNotFound)?;
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
        let memory_id = doc.0.memory_id.clone();
        Ok((memory_id, VersionRecord { version: doc.0, blob }))
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
