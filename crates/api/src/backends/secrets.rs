//! The secret store the configuration selects, each wrapped in its observation.

use std::sync::Arc;

use secrets_aws::{AwsParameterStore, AwsSecretStore};
use shared::hash::SecretKeys;
use shared::ports::secrets::{SecretError, SecretStore};
use store_postgres::{PgBackend, PgSecretStore};

use crate::config::SecretsConfig;
use crate::telemetry::ports::Observed;
use crate::telemetry::Metrics;

/// A secret store, ready or waiting for the record store's pool.
pub(super) enum Secrets {
    /// An AWS store, connected and its KMS key checked.
    Ready(Arc<dyn SecretStore>),
    /// The Postgres store, built on the record store's pool.
    Postgres(SecretKeys),
}

impl Secrets {
    /// Connect the store `config` names; an AWS store checks its KMS key
    /// before the record store's pool opens.
    pub(super) async fn connect(config: &SecretsConfig, metrics: &Arc<Metrics>) -> Result<Self, SecretError> {
        let store: Arc<dyn SecretStore> = match config {
            SecretsConfig::Postgres(keys) => return Ok(Self::Postgres(keys.clone())),
            SecretsConfig::AwsSm(aws) => Arc::new(AwsSecretStore::connect(aws).await?),
            SecretsConfig::AwsSsm(ssm) => Arc::new(AwsParameterStore::connect(ssm).await?),
        };
        Ok(Self::Ready(Arc::new(Observed::new(store, metrics.clone()))))
    }

    /// The store, on `pg` when it is the Postgres one.
    pub(super) fn on(self, pg: &PgBackend, metrics: &Arc<Metrics>) -> Arc<dyn SecretStore> {
        match self {
            Self::Ready(store) => store,
            Self::Postgres(keys) => {
                Arc::new(Observed::new(Arc::new(PgSecretStore::new(pg.clone(), keys)), metrics.clone()))
            }
        }
    }
}
