//! agentd-store-aws — AWS-native persistence (DynamoDB + S3) behind the
//! entity store traits, so agentd runs **stateless across replicas**: all
//! shared state lives in DynamoDB/S3, replicas hold nothing.
//!
//! Emulator-neutral: points at any `:4566` LocalStack-compatible endpoint
//! via `AGENTD_AWS_ENDPOINT`. Tested locally against **Floci** (the
//! token-free MIT LocalStack successor); MiniStack or real AWS work
//! unchanged (drop the endpoint override for real AWS).
//!
//! Data model — document-store on DynamoDB (one table per entity, `pk`
//! partition + `sk` sort + `doc` JSON), S3 for blobs. The multi-replica
//! **exactly-once** guarantee is a DynamoDB conditional write
//! (`attribute_not_exists`) — see [`dynamo::DynamoDoc::put_if_absent`] and
//! the deployment fire claim.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod dynamo;
pub mod entities;

pub use entities::agents::DynamoAgentsStore;
pub use entities::deployments::DynamoDeploymentsStore;
pub use entities::files::DynamoFilesStore;

use std::sync::Arc;

use agentd_blobs::{BlobStore, S3BlobStore};
use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::config::{Credentials, Region};

/// Shared AWS clients + table/bucket naming.
#[derive(Clone)]
pub struct AwsBackend {
    /// DynamoDB document helper (entity metadata).
    pub dynamo: dynamo::DynamoDoc,
    /// Shared S3 blob store (bytes) — same layer the Postgres backend uses.
    pub blobs: Arc<dyn BlobStore>,
    /// Table/bucket name prefix (namespacing, e.g. `agentd`).
    pub prefix: String,
}

impl std::fmt::Debug for AwsBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsBackend")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl AwsBackend {
    /// Connect using ambient AWS config, optionally overriding the endpoint
    /// (`AGENTD_AWS_ENDPOINT`) and region for a local emulator.
    pub async fn connect(prefix: impl Into<String>) -> Result<Self, AwsError> {
        let endpoint = std::env::var("AGENTD_AWS_ENDPOINT").ok();
        let region = std::env::var("AWS_DEFAULT_REGION").unwrap_or_else(|_| "us-east-1".into());

        let mut loader = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region));
        if let Some(ep) = &endpoint {
            // Emulators accept any non-empty credentials.
            loader = loader
                .endpoint_url(ep)
                .credentials_provider(Credentials::new("test", "test", None, None, "agentd-emulator"));
        }
        let shared = loader.load().await;

        let dynamo = dynamo::DynamoDoc::new(aws_sdk_dynamodb::Client::new(&shared));
        let prefix = prefix.into();
        // Bytes go to the shared S3 blob store (reads AGENTD_AWS_ENDPOINT itself).
        let blobs: Arc<dyn BlobStore> = Arc::new(
            S3BlobStore::connect(format!("{prefix}-artifacts"))
                .await
                .map_err(|e| AwsError::S3(e.to_string()))?,
        );

        Ok(Self {
            dynamo,
            blobs,
            prefix,
        })
    }

    /// Fully-qualified table name for an entity (`{prefix}_{entity}`).
    pub fn table(&self, entity: &str) -> String {
        format!("{}_{entity}", self.prefix)
    }
}

/// Errors mapping AWS failures onto store-layer errors.
#[derive(Debug, thiserror::Error)]
pub enum AwsError {
    /// DynamoDB failure.
    #[error("dynamodb: {0}")]
    Dynamo(String),
    /// S3 failure.
    #[error("s3: {0}")]
    S3(String),
    /// Stored document failed to deserialize.
    #[error("decode: {0}")]
    Decode(String),
}
