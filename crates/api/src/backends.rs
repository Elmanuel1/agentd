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
use sandbox_docker::DockerRuntime;
use scheduler_local::LocalScheduler;
use secrets_blob::BlobSecretStore;
use sessions::{SessionEventLog, SessionsStore};
use shared::ports::blobs::BlobStore;
use shared::ports::fire::FireRegistry;
use shared::ports::keys::KeySource;
use shared::ports::secrets::SecretStore;
use shared::ports::{Runtime, SchedulerPort};
use skills::{SkillVersionsStore, SkillsStore};
use store_postgres::{
    PgAgentsStore, PgBackend, PgDeploymentsStore, PgEnvironmentsStore, PgError, PgFilesStore, PgMemoryStore,
    PgSessionEventLog, PgSessionsStore, PgSkillVersionsStore, PgSkillsStore, PgVaultsStore,
};
use vaults::VaultsStore;

use crate::config::{
    BlobsConfig, Config, KeysConfig, RuntimeConfig, SchedulerConfig, SecretsConfig, StoreConfig,
};
use crate::keys::EnvKeySource;

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
            deployments: Arc::new(PgDeploymentsStore::new(backend)),
        }
    }
}

/// One backend per port.
pub struct Backends {
    /// The entity stores.
    pub stores: Stores,
    /// API-key resolution.
    pub keys: Arc<dyn KeySource>,
    /// The durable timer.
    pub scheduler: Arc<dyn SchedulerPort>,
    /// Where the scheduler delivers fires; the modules register their handlers here.
    pub fires: Arc<FireRegistry>,
    /// The sandbox runtime; `None` runs sessions without one.
    pub runtime: Option<Arc<dyn Runtime>>,
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
    /// The sandbox runtime.
    #[error("sandbox: {0}")]
    Sandbox(String),
}

impl Backends {
    /// Connect the backends `config` selects. The local scheduler starts
    /// ticking here.
    pub async fn from_config(config: &Config) -> Result<Self, BootError> {
        let blobs: Arc<dyn BlobStore> = match &config.blobs {
            BlobsConfig::S3(s3) => Arc::new(S3BlobStore::connect(s3).await?),
        };
        let secrets: Arc<dyn SecretStore> = match config.secrets {
            SecretsConfig::Blob => Arc::new(BlobSecretStore::new(blobs.clone())),
        };
        let stores = match &config.store {
            StoreConfig::Postgres { url } => Stores::postgres(PgBackend::connect(url).await?, blobs, secrets),
        };
        let keys: Arc<dyn KeySource> = match &config.keys {
            KeysConfig::Env(keys) => Arc::new(EnvKeySource::new(keys)),
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
            RuntimeConfig::None => None,
            RuntimeConfig::Docker { image } => {
                let docker = bollard::Docker::connect_with_defaults()
                    .map_err(|e| BootError::Sandbox(e.to_string()))?;
                let runtime = DockerRuntime::new(docker, image.clone());
                runtime.prepare().await.map_err(|e| BootError::Sandbox(e.to_string()))?;
                Some(Arc::new(runtime))
            }
        };
        Ok(Self { stores, keys, scheduler, fires, runtime })
    }
}
