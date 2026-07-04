//! `FilesStore` on DynamoDB (metadata) + S3 (bytes).
//!
//! Layout: `{prefix}_files` `pk = workspace`, `sk = file_id`, doc = metadata
//! + scope_id; bytes at S3 `{bucket}/{workspace}/{file_id}`.

use agentd_core::WorkspaceId;
use agentd_files::store::{FilesStore, StoreError, StoredFile};
use agentd_files::types::{FileId, FileMetadata};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::{AwsBackend, AwsError};

fn blob_be(e: agentd_blobs::BlobError) -> StoreError {
    StoreError::Backend(e.to_string())
}

/// The DynamoDB-stored portion (bytes live in S3).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileDoc {
    meta: FileMetadata,
    scope_id: Option<String>,
}

/// DynamoDB + S3 backed files store.
#[derive(Debug, Clone)]
pub struct DynamoFilesStore {
    aws: AwsBackend,
}

impl DynamoFilesStore {
    /// Build; ensures the table exists (the bucket is ensured by the blob
    /// store at connect time).
    pub async fn new(aws: AwsBackend) -> Result<Self, AwsError> {
        aws.dynamo.ensure_table(&aws.table("files")).await?;
        Ok(Self { aws })
    }

    fn table(&self) -> String {
        self.aws.table("files")
    }
    fn key(ws: &WorkspaceId, id: &FileId) -> String {
        format!("{}/{}", ws.0, id.as_str())
    }
}

fn be(e: AwsError) -> StoreError {
    StoreError::Backend(e.to_string())
}

#[async_trait]
impl FilesStore for DynamoFilesStore {
    async fn insert(&self, ws: &WorkspaceId, f: StoredFile) -> Result<(), StoreError> {
        // Bytes → S3, metadata → DynamoDB.
        self.aws
            .blobs
            .put(&Self::key(ws, &f.meta.id), f.bytes.clone())
            .await
            .map_err(blob_be)?;
        let doc = FileDoc {
            meta: f.meta.clone(),
            scope_id: f.scope_id.clone(),
        };
        self.aws
            .dynamo
            .put(&self.table(), &ws.0, f.meta.id.as_str(), &doc)
            .await
            .map_err(be)
    }

    async fn get(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError> {
        let doc: FileDoc = self
            .aws
            .dynamo
            .get(&self.table(), &ws.0, id.as_str())
            .await
            .map_err(be)?
            .ok_or(StoreError::NotFound)?;
        let bytes = self
            .aws
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
        self.aws
            .dynamo
            .delete::<FileDoc>(&self.table(), &ws.0, id.as_str())
            .await
            .map_err(be)?;
        self.aws
            .blobs
            .delete(&Self::key(ws, id))
            .await
            .map_err(blob_be)?;
        Ok(existing)
    }

    async fn list(
        &self,
        ws: &WorkspaceId,
        scope_id: Option<&str>,
    ) -> Result<Vec<StoredFile>, StoreError> {
        let docs: Vec<FileDoc> =
            self.aws.dynamo.query(&self.table(), &ws.0, None).await.map_err(be)?;
        // List is metadata-only in practice (the service maps to `.meta`), so
        // we skip the per-file S3 fetch and return empty bytes.
        Ok(docs
            .into_iter()
            .filter(|d| scope_id.is_none_or(|s| d.scope_id.as_deref() == Some(s)))
            .map(|d| StoredFile {
                meta: d.meta,
                bytes: Bytes::new(),
                scope_id: d.scope_id,
            })
            .collect())
    }
}
