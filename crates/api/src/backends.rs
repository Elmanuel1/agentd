//! The composition root: one backend per port, selected by [`Config`].
//! Adding a backend is a new config value and one match arm here; services
//! and routes see only the ports.

use std::sync::Arc;
use std::time::Duration;

use agents::AgentsStore;
use blobs_s3::{BlobError, S3BlobStore};
use deployments::DeploymentsStore;
use environments::EnvironmentsStore;
use files::store::FilesStore;
use memory::MemoryPersistence;
use organizations::workspaces::WorkspacesStore;
use sandbox_docker::DockerRuntime;

use crate::telemetry::ports::Observed;
use crate::telemetry::{self, Metrics};
use api_keys::ApiKeysService;
use scheduler_local::LocalScheduler;
use secrets_aws::{AwsAdminKeys, AwsSecretStore};
use sessions::{SessionEventLog, SessionsStore};
use shared::ports::blobs::BlobStore;
use shared::ports::fire::FireRegistry;
use shared::ports::keys::{AdminKeySource, KeySource};
use shared::ports::secrets::{SecretError, SecretStore};
use shared::ports::{Runtime, SchedulerPort};
use skills::{SkillVersionsStore, SkillsStore};
use store_postgres::{
    PgAgentsStore, PgApiKeysStore, PgBackend, PgDeploymentsStore, PgEnvironmentsStore, PgError, PgFilesStore,
    PgMemoryStore, PgSessionEventLog, PgSessionsStore, PgSkillVersionsStore, PgSkillsStore, PgVaultsStore,
    PgWorkspacesStore,
};
use vaults::VaultsStore;

use crate::config::{
    BlobsConfig, Config, KeysConfig, RuntimeConfig, SandboxConfig, SchedulerConfig, StoreConfig,
};

/// How often the local scheduler checks for due fires (cron is minute-grained).
const LOCAL_TICK: Duration = Duration::from_secs(10);

/// The entity stores.
#[derive(Clone)]
pub struct Stores {
    /// Agents.
    pub agents: Arc<dyn AgentsStore>,
    /// Environments.
    pub environments: Arc<dyn EnvironmentsStore>,
    /// Vaults.
    pub vaults: Arc<dyn VaultsStore>,
    /// Memory stores, memories and versions.
    pub memory: Arc<dyn MemoryPersistence>,
    /// Files.
    pub files: Arc<dyn FilesStore>,
    /// Skills.
    pub skills: Arc<dyn SkillsStore>,
    /// Skill versions.
    pub skill_versions: Arc<dyn SkillVersionsStore>,
    /// Sessions.
    pub sessions: Arc<dyn SessionsStore>,
    /// Session event logs.
    pub session_events: Arc<dyn SessionEventLog>,
    /// Deployments.
    pub deployments: Arc<dyn DeploymentsStore>,
    /// Workspaces.
    pub workspaces: Arc<dyn WorkspacesStore>,
}

impl std::fmt::Debug for Stores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stores").finish_non_exhaustive()
    }
}

impl Stores {
    /// Every store on Postgres, with bytes in `blobs` and secrets in `secrets`.
    pub fn postgres(backend: PgBackend, blobs: Arc<dyn BlobStore>, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            agents: Arc::new(PgAgentsStore::new(backend.clone())),
            environments: Arc::new(PgEnvironmentsStore::new(backend.clone())),
            vaults: Arc::new(PgVaultsStore::new(backend.clone(), secrets)),
            memory: Arc::new(PgMemoryStore::new(backend.clone(), blobs.clone())),
            files: Arc::new(PgFilesStore::new(backend.clone(), blobs.clone())),
            skills: Arc::new(PgSkillsStore::new(backend.clone(), blobs.clone())),
            skill_versions: Arc::new(PgSkillVersionsStore::new(backend.clone(), blobs)),
            sessions: Arc::new(PgSessionsStore::new(backend.clone())),
            session_events: Arc::new(PgSessionEventLog::new(backend.clone())),
            deployments: Arc::new(PgDeploymentsStore::new(backend.clone())),
            workspaces: Arc::new(PgWorkspacesStore::new(backend)),
        }
    }
}

/// One backend per port.
pub struct Backends {
    /// The entity stores.
    pub stores: Stores,
    /// Workspace key resolution.
    pub keys: Arc<dyn KeySource>,
    /// Admin key resolution.
    pub admin_keys: Arc<dyn AdminKeySource>,
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
        let secrets: Arc<dyn SecretStore> = Arc::new(Observed::new(Arc::new(aws), metrics.clone()));
        let (stores, keys) = match &config.store {
            StoreConfig::Postgres { url, pool } => {
                let observer = telemetry::db::observer(metrics.clone());
                let pg = PgBackend::connect_observed(url, *pool, observer).await?;
                telemetry::db::watch_pool(&opentelemetry::global::meter("agentd"), pg.diesel.clone());
                let keys = ApiKeysService::new(Arc::new(PgApiKeysStore::new(pg.clone())), pepper);
                let keys: Arc<dyn KeySource> = Arc::new(Observed::new(Arc::new(keys), metrics.clone()));
                (Stores::postgres(pg, blobs, secrets), keys)
            }
        };
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
        Ok(Self { stores, keys, admin_keys, scheduler, fires, runtime, sandbox, extra_models, metrics })
    }
}
