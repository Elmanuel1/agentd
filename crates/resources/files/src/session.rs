//! The files API as a session's own work token reaches it: the files the
//! session mounts, which it may read and download though no API key can, and
//! the outputs it saves, scoped to the session. Any other file does not
//! exist for it.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use shared::{ApiError, AuthContext, SessionCaller};

use crate::service::map_store;
use crate::store::StoreError;
use crate::types::FileMetadata;
use crate::FilesService;

/// What the files API asks of sessions.
#[async_trait]
pub trait SessionFiles: Send + Sync {
    /// The ids of the files `session` mounts.
    async fn mounts(&self, ctx: &AuthContext, session: &str) -> Result<Vec<String>, ApiError>;
}

/// The sessions port, as the files routes hold it.
#[derive(Clone)]
pub struct Mounts(pub Arc<dyn SessionFiles>);

impl std::fmt::Debug for Mounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Mounts")
    }
}

impl FilesService {
    /// `GET /v1/files/{id}` from a session's token: one of its outputs or a
    /// file it mounts.
    pub async fn metadata_for(
        &self,
        ctx: &AuthContext,
        mounts: &Mounts,
        caller: &SessionCaller,
        raw_id: &str,
    ) -> Result<FileMetadata, ApiError> {
        let meta = self.metadata(ctx, raw_id).await?;
        if meta.is_output_of(&caller.session)
            || mounts.0.mounts(ctx, &caller.session).await?.iter().any(|f| f == raw_id)
        {
            return Ok(meta);
        }
        Err(map_store(StoreError::NotFound, raw_id))
    }

    /// `GET /v1/files/{id}/content` from a session's token: one of its
    /// outputs, or a file it mounts, which a client cannot download.
    pub async fn download_for(
        &self,
        ctx: &AuthContext,
        mounts: &Mounts,
        caller: &SessionCaller,
        raw_id: &str,
    ) -> Result<(FileMetadata, Bytes), ApiError> {
        let meta = self.metadata_for(ctx, mounts, caller, raw_id).await?;
        Ok((meta, self.bytes(ctx, raw_id).await?))
    }

    /// `POST /v1/files` from a session's token: an output, scoped to the
    /// session whatever `scope_id` says, and downloadable, as live serves
    /// outputs.
    pub async fn upload_for(
        &self,
        ctx: &AuthContext,
        caller: &SessionCaller,
        filename: Option<String>,
        bytes: Bytes,
    ) -> Result<FileMetadata, ApiError> {
        let filename = shared::limits::required("file", filename)?;
        let filename = filename.rsplit(['/', '\\']).next().unwrap_or_default().to_owned();
        self.save_output(ctx, &caller.session, &filename, bytes).await
    }

    /// `DELETE /v1/files/{id}` from a session's token: its own outputs only.
    pub async fn delete_for(
        &self,
        ctx: &AuthContext,
        caller: &SessionCaller,
        raw_id: &str,
    ) -> Result<FileMetadata, ApiError> {
        if !self.metadata(ctx, raw_id).await?.is_output_of(&caller.session) {
            return Err(map_store(StoreError::NotFound, raw_id));
        }
        self.delete(ctx, raw_id).await
    }
}
