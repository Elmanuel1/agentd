//! `FilesStore` on Postgres — metadata columns + `bytea` bytes.

use agentd_core::WorkspaceId;
use agentd_files::store::{FilesStore, StoreError, StoredFile};
use agentd_files::types::{FileId, FileMetadata};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::Row;

use crate::{sqlx_msg, PgBackend};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileDoc {
    meta: FileMetadata,
    scope_id: Option<String>,
}

/// Postgres-backed files store.
#[derive(Debug, Clone)]
pub struct PgFilesStore {
    backend: PgBackend,
}

impl PgFilesStore {
    /// Wrap a pool.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

fn be(e: sqlx::Error) -> StoreError {
    StoreError::Backend(sqlx_msg(e))
}

#[async_trait]
impl FilesStore for PgFilesStore {
    async fn insert(&self, ws: &WorkspaceId, f: StoredFile) -> Result<(), StoreError> {
        let doc = FileDoc {
            meta: f.meta.clone(),
            scope_id: f.scope_id.clone(),
        };
        sqlx::query(
            "insert into files (workspace_id, file_id, filename, scope_id, size_bytes, created_at, doc, bytes)
             values ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(&ws.0)
        .bind(f.meta.id.as_str())
        .bind(&f.meta.filename)
        .bind(&f.scope_id)
        .bind(f.meta.size_bytes as i64)
        .bind(f.meta.created_at.as_datetime())
        .bind(Json(&doc))
        .bind(f.bytes.as_ref())
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError> {
        let row = sqlx::query(
            "select doc, bytes from files where workspace_id = $1 and file_id = $2",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?
        .ok_or(StoreError::NotFound)?;
        let doc: Json<FileDoc> = row.try_get("doc").map_err(be)?;
        let bytes: Vec<u8> = row.try_get("bytes").map_err(be)?;
        Ok(StoredFile {
            meta: doc.0.meta,
            bytes: Bytes::from(bytes),
            scope_id: doc.0.scope_id,
        })
    }

    async fn delete(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError> {
        let existing = self.get(ws, id).await?;
        sqlx::query("delete from files where workspace_id = $1 and file_id = $2")
            .bind(&ws.0)
            .bind(id.as_str())
            .execute(&self.backend.pool)
            .await
            .map_err(be)?;
        Ok(existing)
    }

    async fn list(
        &self,
        ws: &WorkspaceId,
        scope_id: Option<&str>,
    ) -> Result<Vec<StoredFile>, StoreError> {
        // Metadata-only list (bytes omitted); scope filter is SQL.
        let rows: Vec<(Json<FileDoc>,)> = match scope_id {
            Some(s) => sqlx::query_as(
                "select doc from files where workspace_id = $1 and scope_id = $2 order by created_at desc",
            )
            .bind(&ws.0)
            .bind(s)
            .fetch_all(&self.backend.pool)
            .await
            .map_err(be)?,
            None => sqlx::query_as(
                "select doc from files where workspace_id = $1 order by created_at desc",
            )
            .bind(&ws.0)
            .fetch_all(&self.backend.pool)
            .await
            .map_err(be)?,
        };
        Ok(rows
            .into_iter()
            .map(|(d,)| StoredFile {
                meta: d.0.meta,
                bytes: Bytes::new(),
                scope_id: d.0.scope_id,
            })
            .collect())
    }
}
