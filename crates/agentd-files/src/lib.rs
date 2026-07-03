//! agentd-files — the Files support resource (`/v1/files`).
//!
//! Upload via multipart, list (with `scope_id` filter for session outputs),
//! metadata get, raw download, delete. Delete-only lifecycle (no archive).
//! Wire shapes follow the documented Files API; conformance against live
//! uploads is verified by the harness (marked: shape unprobed live).

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod routes;
pub mod store;
pub mod types;

use std::sync::Arc;

use agentd_core::limits::required;
use agentd_core::{ApiError, AuthContext, Timestamp};
use bytes::Bytes;

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
            .insert(
                &ctx.workspace,
                StoredFile {
                    meta: meta.clone(),
                    bytes,
                    scope_id,
                },
            )
            .await
            .map_err(map_store)?;
        Ok(meta)
    }

    /// `GET /v1/files/{id}` — metadata.
    pub async fn metadata(&self, ctx: &AuthContext, raw_id: &str) -> Result<FileMetadata, ApiError> {
        let id = parse_id(raw_id)?;
        self.store
            .get(&ctx.workspace, &id)
            .await
            .map(|f| f.meta)
            .map_err(map_store)
    }

    /// `GET /v1/files/{id}/content` — raw bytes.
    pub async fn download(&self, ctx: &AuthContext, raw_id: &str) -> Result<(FileMetadata, Bytes), ApiError> {
        let id = parse_id(raw_id)?;
        self.store
            .get(&ctx.workspace, &id)
            .await
            .map(|f| (f.meta, f.bytes))
            .map_err(map_store)
    }

    /// `GET /v1/files?scope_id=…`.
    pub async fn list(
        &self,
        ctx: &AuthContext,
        scope_id: Option<&str>,
    ) -> Result<Vec<FileMetadata>, ApiError> {
        Ok(self
            .store
            .list(&ctx.workspace, scope_id)
            .await
            .map_err(map_store)?
            .into_iter()
            .map(|f| f.meta)
            .collect())
    }

    /// `DELETE /v1/files/{id}`.
    pub async fn delete(&self, ctx: &AuthContext, raw_id: &str) -> Result<FileMetadata, ApiError> {
        let id = parse_id(raw_id)?;
        self.store
            .delete(&ctx.workspace, &id)
            .await
            .map(|f| f.meta)
            .map_err(map_store)
    }
}

fn parse_id(raw: &str) -> Result<FileId, ApiError> {
    FileId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("file not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentd_core::{OrgId, WorkspaceId};
    use store::InMemoryFilesStore;

    fn svc() -> FilesService {
        FilesService::new(Arc::new(InMemoryFilesStore::new()))
    }

    fn ctx() -> AuthContext {
        AuthContext {
            org: OrgId("org_t".into()),
            workspace: WorkspaceId("wrkspc_01t".into()),
            api_key_id: "apikey_01t".into(),
        }
    }

    #[tokio::test]
    async fn upload_download_roundtrip() {
        let s = svc();
        let meta = s
            .upload(
                &ctx(),
                Some("data.csv".into()),
                Some("text/csv".into()),
                Bytes::from_static(b"a,b\n1,2\n"),
                None,
            )
            .await
            .unwrap();
        assert!(meta.id.as_str().starts_with("file_01"));
        assert_eq!(meta.size_bytes, 8);
        let (m2, bytes) = s.download(&ctx(), meta.id.as_str()).await.unwrap();
        assert_eq!(m2.filename, "data.csv");
        assert_eq!(&bytes[..], b"a,b\n1,2\n");
    }

    #[tokio::test]
    async fn scope_id_filters_session_outputs() {
        let s = svc();
        s.upload(&ctx(), Some("a".into()), None, Bytes::from_static(b"x"), Some("sesn_01A".into()))
            .await
            .unwrap();
        s.upload(&ctx(), Some("b".into()), None, Bytes::from_static(b"y"), Some("sesn_01B".into()))
            .await
            .unwrap();
        s.upload(&ctx(), Some("c".into()), None, Bytes::from_static(b"z"), None)
            .await
            .unwrap();
        let scoped = s.list(&ctx(), Some("sesn_01A")).await.unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].filename, "a");
        let all = s.list(&ctx(), None).await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn delete_then_404() {
        let s = svc();
        let meta = s
            .upload(&ctx(), Some("f".into()), None, Bytes::from_static(b"x"), None)
            .await
            .unwrap();
        s.delete(&ctx(), meta.id.as_str()).await.unwrap();
        assert_eq!(s.metadata(&ctx(), meta.id.as_str()).await.unwrap_err().status, 404);
    }

    #[tokio::test]
    async fn missing_file_field_is_required_error() {
        let s = svc();
        let e = s
            .upload(&ctx(), None, None, Bytes::new(), None)
            .await
            .unwrap_err();
        assert_eq!(e.message, "file: Field required");
    }

    #[tokio::test]
    async fn workspace_isolation() {
        let s = svc();
        let meta = s
            .upload(&ctx(), Some("f".into()), None, Bytes::from_static(b"x"), None)
            .await
            .unwrap();
        let other = AuthContext {
            org: OrgId("org_t".into()),
            workspace: WorkspaceId("wrkspc_02".into()),
            api_key_id: "apikey_02".into(),
        };
        assert_eq!(s.metadata(&other, meta.id.as_str()).await.unwrap_err().status, 404);
    }
}
