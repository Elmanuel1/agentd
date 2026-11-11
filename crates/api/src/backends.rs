//! The composition root: one backend per port, selected by [`Config`].
//! Adding a backend is a new config value and one match arm here; services
//! and routes see only the ports.

use std::sync::Arc;
use std::time::Duration;

use blobs_s3::{BlobError, S3BlobStore};
use sandbox_docker::DockerRuntime;

mod postgres;
mod stores;

pub use stores::Stores;

use crate::telemetry::ports::Observed;
use crate::telemetry::Metrics;
use api_keys::{ApiKeysService, EnvironmentKeysService};
use scheduler_local::LocalScheduler;
use secrets_aws::{AwsAdminKeys, AwsSecretStore};
use sessions::signals::{LocalSignals, Signals};
use sessions::store::DatabaseLoad;
use shared::ports::blobs::BlobStore;
use shared::ports::fire::FireRegistry;
use shared::ports::keys::{AdminKeySource, EnvironmentKeySource, KeySource};
use shared::ports::secrets::{SecretError, SecretStore};
use shared::ports::{Runtime, SchedulerPort};
use signals_valkey::ValkeySignals;
use store_postgres::PgError;

use crate::config::{
    BlobsConfig, Config, KeysConfig, RuntimeConfig, SandboxConfig, SchedulerConfig, StoreConfig,
};

/// How often the local scheduler checks for due fires (cron is minute-grained).
const LOCAL_TICK: Duration = Duration::from_secs(10);

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
    /// The durable timer.
    pub scheduler: Arc<dyn SchedulerPort>,
    /// Where the scheduler delivers fires; the modules register their handlers here.
    pub fires: Arc<FireRegistry>,
    /// The sandbox runtime. Configuration always gives one; `None` is for tests
    /// that build their backends in code.
    pub runtime: Option<Arc<dyn Runtime>>,
    /// How sandboxes reach agentd and the egress service; set with the runtime.
    pub sandbox: Option<SandboxConfig>,
    /// Model ids agents may name beyond the built-in ones (`AGENTD_MODELS`).
    pub extra_models: Vec<String>,
    /// The instruments every boundary records into.
    pub metrics: Arc<Metrics>,
    /// How replicas tell each other a session changed.
    pub signals: Arc<dyn Signals>,
    /// The database's load, which new streams wait on.
    pub load: Arc<dyn DatabaseLoad>,
}

/// The deployment's own keys, read from Secrets Manager once at boot.
#[derive(Clone)]
pub struct SystemKeys {
    /// Signs work tokens (`<prefix>/system/work_token_key`, or `AGENTD_WORK_TOKEN_KEY`).
    pub work_token_key: Vec<u8>,
    /// Signs `environment_variable` placeholders (`<prefix>/system/egress_key`).
    pub egress_key: Vec<u8>,
    /// agentd's API as a work secret's `api_base_url` names it.
    pub api_base_url: String,
}

impl SystemKeys {
    /// Read the keys from `aws`, each created the first time; with
    /// `AGENTD_WORK_TOKEN_KEY` set, work tokens are signed with it and
    /// `system/work_token_key` is not read.
    pub async fn load(aws: &AwsSecretStore, sandbox: &SandboxConfig) -> Result<Self, SecretError> {
        Ok(Self {
            work_token_key: match &sandbox.work_token_key {
                Some(key) => key.clone(),
                None => aws.work_token_key().await?.to_vec(),
            },
            egress_key: aws.egress_key().await?.to_vec(),
            api_base_url: sandbox.api_base_url.clone(),
        })
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
    /// The sandbox runtime.
    #[error("sandbox: {0}")]
    Sandbox(String),
    /// The Valkey the replicas signal through.
    #[error("signals: {0}")]
    Signals(String),
}

impl Backends {
    /// Connect the backends `config` selects. The local scheduler starts
    /// ticking here.
    pub async fn from_config(config: &Config) -> Result<Self, BootError> {
        let metrics = Arc::new(Metrics::global());
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
        let (stores, load, keys) = match &config.store {
            StoreConfig::Postgres { url, read_url, pool } => {
                let retention = config.streams.retention;
                let pg =
                    postgres::connect(url, read_url.as_deref(), *pool, retention, &metrics, blobs, secrets)
                        .await?;
                let keys = ApiKeysService::new(pg.stores.api_keys.clone(), pepper.clone());
                let keys: Arc<dyn KeySource> = Arc::new(Observed::new(Arc::new(keys), metrics.clone()));
                (pg.stores, pg.load, keys)
            }
        };
        let signals: Arc<dyn Signals> = match &config.streams.valkey_url {
            Some(url) => {
                Arc::new(ValkeySignals::connect(url).await.map_err(|e| BootError::Signals(e.to_string()))?)
            }
            None => Arc::new(LocalSignals::default()),
        };
        let environment_keys = EnvironmentKeysService::new(stores.environment_keys.clone(), pepper)
            .with_runtime_key(&aws.runtime_key().await?);
        let system = SystemKeys::load(&aws, &config.sandbox).await?;
        let fires = Arc::new(FireRegistry::new());
        let scheduler: Arc<dyn SchedulerPort> = match config.scheduler {
            SchedulerConfig::Local => {
                let local = Arc::new(LocalScheduler::new(fires.clone()));
                drop(local.clone().spawn(LOCAL_TICK));
                local
            }
        };
        let runtime: Option<Arc<dyn Runtime>> = match &config.runtime {
            RuntimeConfig::Docker { image, limits, network } => {
                let docker = bollard::Docker::connect_with_defaults()
                    .map_err(|e| BootError::Sandbox(e.to_string()))?;
                let runtime = DockerRuntime::new(docker, image.clone(), *limits, network.clone());
                runtime.prepare().await.map_err(|e| BootError::Sandbox(e.to_string()))?;
                Some(Arc::new(Observed::new(Arc::new(runtime), metrics.clone())))
            }
        };
        let extra_models = config.extra_models.clone();
        let sandbox = runtime.as_ref().map(|_| config.sandbox.clone());
        let environment_keys: Arc<dyn EnvironmentKeySource> = Arc::new(environment_keys);
        Ok(Self {
            stores,
            keys,
            admin_keys,
            environment_keys,
            system,
            scheduler,
            fires,
            runtime,
            sandbox,
            extra_models,
            metrics,
            signals,
            load,
        })
    }
}
