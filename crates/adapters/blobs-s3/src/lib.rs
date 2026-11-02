//! blobs-s3: the S3 implementation of [`BlobStore`].
//!
//! [`S3BlobStore`] targets any S3-compatible endpoint: real AWS S3, MinIO, or
//! an emulator such as LocalStack. Credentials always come from the AWS
//! default chain (env keys, profile, web identity, ECS task or EC2 instance
//! role), optionally exchanged for an assumed role.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use aws_config::environment::EnvironmentVariableRegionProvider;
use aws_config::meta::region::RegionProviderChain;
use aws_config::profile::ProfileFileRegionProvider;
use aws_config::sts::AssumeRoleProvider;
use aws_config::BehaviorVersion;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::head_bucket::HeadBucketError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{BucketLocationConstraint, CreateBucketConfiguration};
use bytes::Bytes;
pub use shared::ports::blobs::{BlobError, BlobStore};

/// Where and how to reach the bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Config {
    /// The bucket every blob lives in.
    pub bucket: String,
    /// The region; `None` takes `AWS_REGION`, `AWS_DEFAULT_REGION` or the
    /// profile's, else `us-east-1`.
    pub region: Option<String>,
    /// A custom endpoint (MinIO, LocalStack); `None` is AWS S3.
    pub endpoint: Option<String>,
    /// Address the bucket in the path rather than the host name.
    pub path_style: bool,
    /// A role to assume with the default-chain credentials.
    pub role_arn: Option<String>,
    /// Create the bucket at connect when it does not exist.
    pub create_bucket: bool,
    /// The location constraint a created bucket gets. `None` is the region on
    /// AWS S3 (none in us-east-1) and none on a custom endpoint, whose
    /// locations are named their own way or refused.
    pub location_constraint: Option<String>,
}

impl S3Config {
    /// `bucket` on AWS S3 in the default-chain region, with no bucket creation.
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            region: None,
            endpoint: None,
            path_style: false,
            role_arn: None,
            create_bucket: false,
            location_constraint: None,
        }
    }
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
    /// Build the client from `config`, and create the bucket when the config
    /// asks for it.
    pub async fn connect(config: &S3Config) -> Result<Self, BlobError> {
        // The region chain skips IMDS so a boot off EC2 never waits on it;
        // ECS and Lambda set AWS_REGION.
        let region = RegionProviderChain::first_try(config.region.clone().map(Region::new))
            .or_else(EnvironmentVariableRegionProvider::new())
            .or_else(ProfileFileRegionProvider::new())
            .or_else(Region::from_static("us-east-1"));
        let mut shared = aws_config::defaults(BehaviorVersion::latest()).region(region).load().await;
        if let Some(role_arn) = &config.role_arn {
            let role =
                AssumeRoleProvider::builder(role_arn).session_name("agentd").configure(&shared).build();
            shared = shared
                .into_builder()
                .credentials_provider(aws_sdk_s3::config::SharedCredentialsProvider::new(role.await))
                .build();
        }
        // The endpoint applies to S3 only: STS keeps its own.
        let mut conf = aws_sdk_s3::config::Builder::from(&shared).force_path_style(config.path_style);
        if let Some(endpoint) = &config.endpoint {
            conf = conf.endpoint_url(endpoint);
        }
        let conf = conf.build();
        let store = Self { client: aws_sdk_s3::Client::from_conf(conf), bucket: config.bucket.clone() };
        if config.create_bucket {
            let location = config.location_constraint.clone().or_else(|| {
                // AWS S3 refuses a create outside us-east-1 without the region.
                let region = store.client.config().region()?.as_ref().to_owned();
                (config.endpoint.is_none() && region != "us-east-1").then_some(region)
            });
            store.ensure_bucket(location).await?;
        }
        Ok(store)
    }

    async fn ensure_bucket(&self, location: Option<String>) -> Result<(), BlobError> {
        match self.client.head_bucket().bucket(&self.bucket).send().await {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(se))
                if matches!(se.err(), HeadBucketError::NotFound(_)) || se.raw().status().as_u16() == 404 =>
            {
                let location = location.map(|l| {
                    CreateBucketConfiguration::builder()
                        .location_constraint(BucketLocationConstraint::from(l.as_str()))
                        .build()
                });
                self.client
                    .create_bucket()
                    .bucket(&self.bucket)
                    .set_create_bucket_configuration(location)
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
