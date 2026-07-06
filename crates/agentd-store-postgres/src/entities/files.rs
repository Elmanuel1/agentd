//! `FilesStore` — metadata in Postgres (Diesel), **bytes in S3** (via
//! agentd-blobs). The database never holds file contents. Key `{ws}/{file_id}`.

use std::sync::Arc;

use agentd_blobs::BlobStore;
use agentd_core::WorkspaceId;
use agentd_files::store::{FilesStore, StoreError, StoredFile};
use agentd_files::types::{FileId, FileMetadata};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use serde::{Deserialize, Serialize};

use crate::schema::files;
use crate::{blob_be, diesel_be, parse_doc, pool_be, to_doc, PgBackend};

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
    /// Build with a Postgres backend (metadata) and a blob store (bytes).
    pub fn new(backend: PgBackend, blobs: Arc<dyn BlobStore>) -> Self {
        Self { backend, blobs }
    }

    fn key(ws: &WorkspaceId, id: &FileId) -> String {
        format!("{}/{}", ws.as_str(), id.as_str())
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = files)]
struct NewFile {
    workspace_id: String,
    file_id: String,
    filename: String,
    scope_id: Option<String>,
    size_bytes: i64,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
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
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(files::table)
            .values(NewFile {
                workspace_id: ws.as_str().to_owned(),
                file_id: f.meta.id.as_str().to_owned(),
                filename: f.meta.filename.clone(),
                scope_id: f.scope_id.clone(),
                size_bytes: f.meta.size_bytes as i64,
                created_at: f.meta.created_at.as_datetime(),
                doc: to_doc(&doc)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = files::table
            .filter(files::workspace_id.eq(ws.as_str()))
            .filter(files::file_id.eq(id.as_str()))
            .select(files::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        let doc: FileDoc = parse_doc(doc.ok_or(StoreError::NotFound)?)?;
        let bytes = self.blobs.get(&Self::key(ws, id)).await.map_err(blob_be)?;
        Ok(StoredFile {
            meta: doc.meta,
            bytes,
            scope_id: doc.scope_id,
        })
    }

    async fn delete(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError> {
        let existing = self.get(ws, id).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::delete(
            files::table
                .filter(files::workspace_id.eq(ws.as_str()))
                .filter(files::file_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        self.blobs.delete(&Self::key(ws, id)).await.map_err(blob_be)?;
        Ok(existing)
    }

    async fn list(
        &self,
        ws: &WorkspaceId,
        scope_id: Option<&str>,
    ) -> Result<Vec<StoredFile>, StoreError> {
        // Metadata-only list — no S3 round-trips; scope filter is SQL.
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let mut q = files::table
            .filter(files::workspace_id.eq(ws.as_str()))
            .into_boxed();
        if let Some(s) = scope_id {
            q = q.filter(files::scope_id.eq(s.to_owned()));
        }
        let docs: Vec<serde_json::Value> = q
            .order(files::created_at.desc())
            .select(files::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter()
            .map(|d| {
                let doc: FileDoc = parse_doc(d)?;
                Ok(StoredFile {
                    meta: doc.meta,
                    bytes: Bytes::new(),
                    scope_id: doc.scope_id,
                })
            })
            .collect()
    }
}
