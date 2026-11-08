//! The files API as a session's own work token reaches it: the session's
//! harness saves the session's outputs, replaces them, and reads the files
//! the session mounts.

use bytes::Bytes;
use shared::{ApiError, AuthContext, SessionCaller, Timestamp};

use crate::service::{file_name, map_store};
use crate::store::StoreError;
use crate::types::{FileId, FileMetadata, FileScope, FileTag};
use crate::FilesService;

impl FilesService {
    /// `POST /v1/files` from a session's token: a file the session wrote to
    /// `/mnt/session/outputs/`, scoped to the session and downloadable, as
    /// live serves outputs.
    pub async fn save_output(
        &self,
        ctx: &AuthContext,
        caller: &SessionCaller,
        filename: Option<String>,
        bytes: Bytes,
    ) -> Result<FileMetadata, ApiError> {
        let filename = file_name(filename)?;
        let mime = match filename.rsplit_once('.').map(|(_, ext)| ext) {
            Some("json") => "application/json",
            Some("txt" | "md" | "csv") => "text/plain",
            Some("pdf") => "application/pdf",
            _ => "application/octet-stream",
        };
        let meta = FileMetadata {
            created_at: Timestamp::now(),
            downloadable: true,
            expires_at: None,
            filename,
            id: FileId::generate(),
            mime_type: mime.into(),
            size_bytes: bytes.len() as u64,
            tag: FileTag::File,
            scope: Some(FileScope::Session { id: caller.session.clone() }),
        };
        self.insert(ctx, meta, bytes, Some(caller.session.clone())).await
    }

    /// `GET /v1/files/{id}/content` from a session's token: also a file the
    /// session mounts or grades against, which a client cannot download.
    pub async fn download_for(
        &self,
        ctx: &AuthContext,
        caller: &SessionCaller,
        raw_id: &str,
    ) -> Result<(FileMetadata, Bytes), ApiError> {
        if !self.sessions.reads(ctx, &caller.session, raw_id).await? {
            return self.download(ctx, raw_id).await;
        }
        let file = self.live(ctx, raw_id).await?;
        Ok((file.meta, self.bytes(ctx, raw_id).await?))
    }

    /// `DELETE /v1/files/{id}` from a session's token: the session's own
    /// outputs only; any other file does not exist for it.
    pub async fn delete_output(
        &self,
        ctx: &AuthContext,
        caller: &SessionCaller,
        raw_id: &str,
    ) -> Result<FileMetadata, ApiError> {
        let meta = self.metadata(ctx, raw_id).await?;
        let own = matches!(&meta.scope, Some(FileScope::Session { id }) if *id == caller.session);
        if !own {
            return Err(map_store(StoreError::NotFound, raw_id));
        }
        self.delete(ctx, raw_id).await
    }
}
