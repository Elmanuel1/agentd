//! files — the Files support resource (`/v1/files`).
//!
//! Upload via multipart, list (with `scope_id` filter for session outputs),
//! metadata get, raw download, delete. Delete-only lifecycle (no archive).
//! Wire shapes follow the documented Files API; conformance against live
//! uploads is verified by the harness (marked: shape unprobed live).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod routes;
pub mod store;
pub mod types;

use std::sync::Arc;

use bytes::Bytes;
use shared::limits::required;
use shared::module::BodyLimit;
use shared::{ApiError, AuthContext, IdCursor, Module, Timestamp};

use store::{FilesStore, StoreError, StoredFile};
use types::{FileId, FileMetadata, FileTag};

/// Files service.
pub struct FilesService {
    store: Arc<dyn FilesStore>,
}

impl std::fmt::Debug for FilesService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesService").finish_non_exhaustive()
    }
}

impl FilesService {
    /// Build with a store.
    pub fn new(store: Arc<dyn FilesStore>) -> Self {
        Self { store }
    }

    /// `POST /v1/files` (multipart) — filename + bytes.
    pub async fn upload(
        &self,
        ctx: &AuthContext,
        filename: Option<String>,
        mime_type: Option<String>,
        bytes: Bytes,
        scope_id: Option<String>,
    ) -> Result<FileMetadata, ApiError> {
        let filename = required("file", filename)?;
        let meta = FileMetadata {
            created_at: Timestamp::now(),
            downloadable: true,
            filename,
            id: FileId::generate(),
            mime_type: mime_type.unwrap_or_else(|| "application/octet-stream".into()),
            size_bytes: bytes.len() as u64,
            tag: FileTag::File,
        };
        self.store
            .insert(&ctx.workspace, StoredFile { meta: meta.clone(), bytes, scope_id })
            .await
            .map_err(map_store)?;
        Ok(meta)
    }

    /// `GET /v1/files/{id}` — metadata.
    pub async fn metadata(&self, ctx: &AuthContext, raw_id: &str) -> Result<FileMetadata, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.get(&ctx.workspace, &id).await.map(|f| f.meta).map_err(map_store)
    }

    /// `GET /v1/files/{id}/content` — raw bytes.
    pub async fn download(&self, ctx: &AuthContext, raw_id: &str) -> Result<(FileMetadata, Bytes), ApiError> {
        let id = parse_id(raw_id)?;
        self.store.get(&ctx.workspace, &id).await.map(|f| (f.meta, f.bytes)).map_err(map_store)
    }

    /// `GET /v1/files?scope_id=…` — one id-cursor page + `has_more`.
    pub async fn list(
        &self,
        ctx: &AuthContext,
        scope_id: Option<&str>,
        cursor: &IdCursor,
    ) -> Result<(Vec<FileMetadata>, bool), ApiError> {
        let (files, has_more) = self.store.list(&ctx.workspace, scope_id, cursor).await.map_err(map_store)?;
        Ok((files.into_iter().map(|f| f.meta).collect(), has_more))
    }

    /// `DELETE /v1/files/{id}`.
    pub async fn delete(&self, ctx: &AuthContext, raw_id: &str) -> Result<FileMetadata, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.delete(&ctx.workspace, &id).await.map(|f| f.meta).map_err(map_store)
    }
}

fn parse_id(raw: &str) -> Result<FileId, ApiError> {
    FileId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("file not found"),
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}

/// The files module: its routes, with the Files API's 500 MB body cap.
pub fn module(service: Arc<FilesService>) -> Module {
    Module::new(routes::router(), service).with_body_limit(BodyLimit { api: "Files", max_mb: 500 })
}
