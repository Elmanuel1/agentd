//! The stores outside Postgres a CLI command removes from: the secret store, the blob store and
//! the scheduler, as `config` selects them.

use std::sync::Arc;

use blobs_s3::S3BlobStore;
use shared::ports::SchedulerPort;
use shared::ports::blobs::BlobStore;
use shared::ports::secrets::SecretStore;
use store_postgres::PgBackend;

use super::BootError;
use super::scheduler::Sink;
use super::secrets::Secrets;
use crate::config::{BlobsConfig, Config};
use crate::telemetry::Metrics;

/// The secret store, blob store and scheduler `config` selects, over `pg`.
pub(crate) async fn stores(
    config: &Config,
    pg: &PgBackend,
) -> Result<(Arc<dyn SecretStore>, Arc<dyn BlobStore>, Arc<dyn SchedulerPort>), BootError> {
    let metrics = Arc::new(Metrics::global());
    let secrets = Secrets::connect(&config.secrets, &metrics).await?.on(pg, &metrics);
    let blobs: Arc<dyn BlobStore> = match &config.blobs {
        BlobsConfig::S3(s3) => Arc::new(S3BlobStore::connect(s3).await?),
    };
    let scheduler = Sink::connect(&config.scheduler, pg).await?.port();
    Ok((secrets, blobs, scheduler))
}
