//! S3 blob helper — raw bytes for files and (later) memory content / session
//! workspace snapshots.

use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::head_bucket::HeadBucketError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use bytes::Bytes;

use crate::AwsError;

/// Uniform S3 blob access.
#[derive(Clone)]
pub struct S3Blobs {
    client: Client,
}

impl S3Blobs {
    /// Wrap a client.
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Create the bucket if it doesn't exist (idempotent).
    pub async fn ensure_bucket(&self, bucket: &str) -> Result<(), AwsError> {
        match self.client.head_bucket().bucket(bucket).send().await {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(se)) if matches!(se.err(), HeadBucketError::NotFound(_)) => {
                self.client
                    .create_bucket()
                    .bucket(bucket)
                    .send()
                    .await
                    .map_err(|e| AwsError::S3(e.to_string()))?;
                Ok(())
            }
            // Some emulators return a plain 404 without the typed NotFound.
            Err(SdkError::ServiceError(se)) if se.raw().status().as_u16() == 404 => {
                self.client
                    .create_bucket()
                    .bucket(bucket)
                    .send()
                    .await
                    .map_err(|e| AwsError::S3(e.to_string()))?;
                Ok(())
            }
            Err(e) => Err(AwsError::S3(e.to_string())),
        }
    }

    /// Put an object.
    pub async fn put(&self, bucket: &str, key: &str, bytes: Bytes) -> Result<(), AwsError> {
        self.client
            .put_object()
            .bucket(bucket)
            .key(key)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|e| AwsError::S3(e.to_string()))?;
        Ok(())
    }

    /// Get an object.
    pub async fn get(&self, bucket: &str, key: &str) -> Result<Bytes, AwsError> {
        let out = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| AwsError::S3(e.to_string()))?;
        let data = out
            .body
            .collect()
            .await
            .map_err(|e| AwsError::S3(e.to_string()))?;
        Ok(data.into_bytes())
    }

    /// Delete an object.
    pub async fn delete(&self, bucket: &str, key: &str) -> Result<(), AwsError> {
        self.client
            .delete_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| AwsError::S3(e.to_string()))?;
        Ok(())
    }
}
