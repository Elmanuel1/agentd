//! Bucket creation against the S3 emulator at `AGENTD_AWS_ENDPOINT`.
#![allow(clippy::unwrap_used)]

use blobs_s3::{S3BlobStore, S3Config};
use bytes::Bytes;
use shared::ports::blobs::BlobStore;

/// Outside us-east-1 the create carries the region as its location
/// constraint; without it S3 refuses the create.
#[tokio::test]
async fn a_bucket_is_created_outside_us_east_1() {
    let Ok(endpoint) = std::env::var("AGENTD_AWS_ENDPOINT") else {
        return;
    };
    let bucket = format!("agentdregion{}", std::process::id());
    let config = S3Config {
        endpoint: Some(endpoint),
        path_style: true,
        create_bucket: true,
        region: Some("eu-west-1".into()),
        ..S3Config::new(bucket)
    };
    let store = S3BlobStore::connect(&config).await.unwrap();
    store.put("k", Bytes::from_static(b"v")).await.unwrap();
    assert_eq!(&store.get("k").await.unwrap()[..], b"v");
}
