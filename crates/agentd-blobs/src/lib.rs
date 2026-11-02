//! agentd-blobs — object storage behind a [`BlobStore`] port.
//!
//! Bytes (file contents, session workspace snapshots, large memory content)
//! **never** live in the record database — they belong in object storage.
//! The record store keeps only metadata and the blob key.
//!
//! [`S3BlobStore`] targets any S3-compatible endpoint: real AWS S3, MinIO,
//! or an S3 emulator such as LocalStack (via `AGENTD_AWS_ENDPOINT`).

#![forbid(unsafe_code)]

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::head_bucket::HeadBucketError;
use aws_sdk_s3::primitives::ByteStream;
use bytes::Bytes;

/// Blob-store failure.
#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    /// The object does not exist.
    #[error("blob not found: {0}")]
    NotFound(String),
    /// Backend/transport failure.
    #[error("blob backend: {0}")]
    Backend(String),
}

/// Object storage: put/get/delete by key, within one bucket.
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Store bytes at `key` (overwrites).
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), BlobError>;
    /// Fetch bytes at `key`.
    async fn get(&self, key: &str) -> Result<Bytes, BlobError>;
    /// Delete `key` (idempotent).
    async fn delete(&self, key: &str) -> Result<(), BlobError>;
}

/// S3-backed blob store bound to one bucket.
#[derive(Clone)]
pub struct S3BlobStore {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl std::fmt::Debug for S3BlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3BlobStore").field("bucket", &self.bucket).finish_non_exhaustive()
    }
}

impl S3BlobStore {
    /// Connect and ensure the bucket exists. `endpoint` overrides the AWS
    /// endpoint for a local emulator (leave `None` for real AWS).
    pub async fn connect(bucket: impl Into<String>) -> Result<Self, BlobError> {
        let endpoint = std::env::var("AGENTD_AWS_ENDPOINT").ok();
        let region = std::env::var("AWS_DEFAULT_REGION").unwrap_or_else(|_| "us-east-1".into());
        let mut loader = aws_config::defaults(BehaviorVersion::latest()).region(Region::new(region));
        if let Some(ep) = &endpoint {
            loader = loader.endpoint_url(ep).credentials_provider(Credentials::new(
                "test",
                "test",
                None,
                None,
                "agentd-blobs",
            ));
        }
        let shared = loader.load().await;
        let conf = aws_sdk_s3::config::Builder::from(&shared).force_path_style(endpoint.is_some()).build();
        let store = Self { client: aws_sdk_s3::Client::from_conf(conf), bucket: bucket.into() };
        store.ensure_bucket().await?;
        Ok(store)
    }

    async fn ensure_bucket(&self) -> Result<(), BlobError> {
        match self.client.head_bucket().bucket(&self.bucket).send().await {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(se))
                if matches!(se.err(), HeadBucketError::NotFound(_)) || se.raw().status().as_u16() == 404 =>
            {
                self.client
                    .create_bucket()
                    .bucket(&self.bucket)
                    .send()
                    .await
                    .map_err(|e| BlobError::Backend(e.to_string()))?;
                Ok(())
            }
            Err(e) => Err(BlobError::Backend(e.to_string())),
        }
    }
}

#[async_trait]
impl BlobStore for S3BlobStore {
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), BlobError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|e| BlobError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Bytes, BlobError> {
        let out =
            self.client.get_object().bucket(&self.bucket).key(key).send().await.map_err(|e| match &e {
                SdkError::ServiceError(se) if se.err().is_no_such_key() => {
                    BlobError::NotFound(key.to_owned())
                }
                _ => BlobError::Backend(e.to_string()),
            })?;
        let data = out.body.collect().await.map_err(|e| BlobError::Backend(e.to_string()))?;
        Ok(data.into_bytes())
    }

    async fn delete(&self, key: &str) -> Result<(), BlobError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| BlobError::Backend(e.to_string()))?;
        Ok(())
    }
}
