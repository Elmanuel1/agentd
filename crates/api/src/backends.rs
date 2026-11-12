//! The composition root: one backend per port, selected by [`Config`].
//! Adding a backend is a new config value and one match arm here; services
//! and routes see only the ports.

use std::sync::Arc;

use blobs_s3::{BlobError, S3BlobStore};

mod postgres;
mod stores;

pub use stores::Stores;

use crate::telemetry::ports::Observed;
use crate::telemetry::Metrics;
use api_keys::{ApiKeysService, EnvironmentKeysService};
use secrets_aws::{AwsAdminKeys, AwsSecretStore};
use sessions::signals::Signals;
use sessions::store::DatabaseLoad;
use shared::hash::KeyRing;
use shared::ports::blobs::BlobStore;
use shared::ports::fire::FireRegistry;
use shared::ports::keys::{AdminKeySource, EnvironmentKeySource, KeySource};
use shared::ports::secrets::{SecretError, SecretStore};
use signals_valkey::{ValkeyChanges, ValkeySignals};
use store_postgres::PgError;
use store_postgres::ScheduleOutbox;

use crate::config::{BlobsConfig, Config, KeysConfig, Role, StoreConfig, WorkConfig};

/// One backend per port.
pub struct Backends {
    /// The entity stores.
    pub stores: Stores,
    /// Workspace key resolution.
    pub keys: Arc<dyn KeySource>,
    /// Admin key resolution.
    pub admin_keys: Arc<dyn AdminKeySource>,
    /// Environment key resolution, agentd's runtime key among them.
    pub environment_keys: Arc<dyn EnvironmentKeySource>,
    /// The keys work tokens and placeholders are signed with.
    pub system: SystemKeys,
    /// Where the scheduler delivers fires; the modules register their handlers here.
    pub fires: Arc<FireRegistry>,
    /// Model ids agents may name beyond the built-in ones (`AGENTD_MODELS`).
    pub extra_models: Vec<String>,
    /// The instruments every boundary records into.
    pub metrics: Arc<Metrics>,
    /// How replicas tell each other a session changed.
    pub signals: Arc<dyn Signals>,
    /// The database's load, which new streams wait on.
    pub load: Arc<dyn DatabaseLoad>,
    /// What this replica runs; a `loop` replica serves no route.
    pub role: Role,
}

/// The deployment's own keys, from its configuration.
#[derive(Clone)]
pub struct SystemKeys {
    /// Signs and verifies work tokens.
    pub work_tokens: KeyRing,
    /// Signs and verifies `environment_variable` placeholders.
    pub placeholders: KeyRing,
    /// agentd's API as a work secret's `api_base_url` names it.
    pub api_base_url: String,
}

impl From<&WorkConfig> for SystemKeys {
    fn from(work: &WorkConfig) -> Self {
        Self {
            work_tokens: work.work_tokens.clone(),
            placeholders: work.placeholders.clone(),
            api_base_url: work.api_base_url.clone(),
        }
    }
}

impl std::fmt::Debug for SystemKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemKeys").field("api_base_url", &self.api_base_url).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Backends {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backends").field("fires", &self.fires).finish_non_exhaustive()
    }
}

/// A backend that could not be reached at boot.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// The record store.
    #[error("store: {0}")]
    Store(#[from] PgError),
    /// The blob store.
    #[error("blobs: {0}")]
    Blobs(#[from] BlobError),
    /// The secret store, or its KMS key.
    #[error("secrets: {0}")]
    Secrets(#[from] SecretError),
    /// The Valkey the replicas signal through.
    #[error("signals: {0}")]
    Signals(String),
    /// The loop's model providers or budget.
    #[error("loop: {0}")]
    Loop(String),
}

impl Backends {
    /// Connect the backends `config` selects. The scheduler and its outbox
    /// relay start here.
    pub async fn from_config(config: &Config) -> Result<Self, BootError> {
        let metrics = Arc::new(Metrics::global());
        let role = config.agent_loop.role;
        let blobs: Arc<dyn BlobStore> = match &config.blobs {
            BlobsConfig::S3(s3) => Arc::new(S3BlobStore::connect(s3).await?),
        };
        let blobs: Arc<dyn BlobStore> = Arc::new(Observed::new(blobs, metrics.clone()));
        let aws = AwsSecretStore::connect(&config.secrets).await?;
        let KeysConfig::Aws = config.keys;
        let pepper = Arc::new(aws.api_key_pepper().await?);
        let admin = Arc::new(AwsAdminKeys::new(aws.clone(), pepper.clone()));
        let admin_keys: Arc<dyn AdminKeySource> = Arc::new(Observed::new(admin, metrics.clone()));
        let secrets: Arc<dyn SecretStore> = Arc::new(Observed::new(Arc::new(aws.clone()), metrics.clone()));
        let fires = Arc::new(FireRegistry::new());
        let (stores, load, keys) = match &config.store {
            StoreConfig::Postgres { url, read_url, pool } => {
                let retention = config.streams.retention.filter(|_| role.serves_api());
                let s = secrets.clone();
                let pg =
                    postgres::connect(url, read_url.as_deref(), *pool, retention, &metrics, blobs, s).await?;
                let scheduler = postgres::schedule(&pg.backend, &fires);
                let feed = ValkeyChanges::new(
                    url,
                    &config.streams.changes_prefix,
                    &format!("agentd-{}", uuid::Uuid::new_v4()),
                )
                .map_err(|e| BootError::Signals(e.to_string()))?;
                drop(feed.spawn(vec![Arc::new(ScheduleOutbox::new(Arc::new(scheduler)))]));
                let keys = ApiKeysService::new(pg.stores.api_keys.clone(), pepper.clone());
                let keys: Arc<dyn KeySource> = Arc::new(Observed::new(Arc::new(keys), metrics.clone()));
                (pg.stores, pg.load, keys)
            }
        };
        let url = &config.streams.valkey_url;
        let signals: Arc<dyn Signals> =
            Arc::new(ValkeySignals::connect(url).map_err(|e| BootError::Signals(e.to_string()))?);
        let environment_keys = EnvironmentKeysService::new(stores.environment_keys.clone(), pepper)
            .with_runtime_key(&aws.runtime_key().await?);
        let system = SystemKeys::from(&config.work);
        if role.runs_loop() {
            drop(
                crate::agent_loop::spawn(config, &stores, signals.clone(), secrets.as_ref(), &system).await?,
            );
        }
        let extra_models = config.extra_models.clone();
        let environment_keys: Arc<dyn EnvironmentKeySource> = Arc::new(environment_keys);
        Ok(Self {
            stores,
            keys,
            admin_keys,
            environment_keys,
            system,
            fires,
            extra_models,
            metrics,
            signals,
            load,
            role,
        })
    }
}
