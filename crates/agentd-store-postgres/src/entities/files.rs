//! `FilesStore` — metadata in Postgres, **bytes in S3** (via agentd-blobs).
//!
//! The database never holds file contents. Object key: `{workspace}/{file_id}`.

use std::sync::Arc;

use agentd_blobs::BlobStore;
use agentd_core::WorkspaceId;
use agentd_files::store::{FilesStore, StoreError, StoredFile};
use agentd_files::types::{FileId, FileMetadata};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sqlx::types::Json;

use crate::{sqlx_msg, PgBackend};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileDoc {
    meta: FileMetadata,
    scope_id: Option<String>,
}

/// Postgres metadata + S3 bytes.
#[derive(Clone)]
pub struct PgFilesStore {
    backend: PgBackend,
    blobs: Arc<dyn BlobStore>,
}

impl std::fmt::Debug for PgFilesStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgFilesStore").finish_non_exhaustive()
    }
}

impl PgFilesStore {
    /// Build with a Postgres pool (metadata) and a blob store (bytes).
    pub fn new(backend: PgBackend, blobs: Arc<dyn BlobStore>) -> Self {
        Self { backend, blobs }
    }

    fn key(ws: &WorkspaceId, id: &FileId) -> String {
        format!("{}/{}", ws.0, id.as_str())
    }
}

fn be(e: sqlx::Error) -> StoreError {
    StoreError::Backend(sqlx_msg(e))
}
fn blob_be(e: agentd_blobs::BlobError) -> StoreError {
    StoreError::Backend(e.to_string())
}

#[async_trait]
impl FilesStore for PgFilesStore {
    async fn insert(&self, ws: &WorkspaceId, f: StoredFile) -> Result<(), StoreError> {
        // Bytes → S3 first, then metadata → Postgres.
        self.blobs
            .put(&Self::key(ws, &f.meta.id), f.bytes.clone())
            .await
            .map_err(blob_be)?;
        let doc = FileDoc {
            meta: f.meta.clone(),
            scope_id: f.scope_id.clone(),
        };
        sqlx::query(
            "insert into files (workspace_id, file_id, filename, scope_id, size_bytes, created_at, doc)
             values ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&ws.0)
        .bind(f.meta.id.as_str())
        .bind(&f.meta.filename)
        .bind(&f.scope_id)
        .bind(f.meta.size_bytes as i64)
        .bind(f.meta.created_at.as_datetime())
        .bind(Json(&doc))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError> {
        let row: Option<(Json<FileDoc>,)> =
            sqlx::query_as("select doc from files where workspace_id = $1 and file_id = $2")
                .bind(&ws.0)
                .bind(id.as_str())
                .fetch_optional(&self.backend.pool)
                .await
                .map_err(be)?;
        let doc = row.map(|(d,)| d.0).ok_or(StoreError::NotFound)?;
        let bytes = self
            .blobs
            .get(&Self::key(ws, id))
            .await
            .map_err(blob_be)?;
        Ok(StoredFile {
            meta: doc.meta,
            bytes,
            scope_id: doc.scope_id,
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
        self.blobs.delete(&Self::key(ws, id)).await.map_err(blob_be)?;
        Ok(existing)
    }

    async fn list(
        &self,
        ws: &WorkspaceId,
        scope_id: Option<&str>,
    ) -> Result<Vec<StoredFile>, StoreError> {
        // Metadata-only list — no S3 round-trips; scope filter is SQL.
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
