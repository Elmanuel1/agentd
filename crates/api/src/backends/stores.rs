//! The entity stores, one backend per store port.

use std::sync::Arc;

use agents::AgentsStore;
use api_keys::{ApiKeysStore, EnvironmentKeysStore};
use deployments::DeploymentsStore;
use environments::EnvironmentsStore;
use files::store::FilesStore;
use memory::MemoryPersistence;
use organizations::workspaces::WorkspacesStore;
use sessions::steps::Steps;
use sessions::{SessionEventLog, SessionEventReads, SessionsStore};
use shared::ports::blobs::BlobStore;
use shared::ports::secrets::SecretStore;
use skills::{SkillVersionsStore, SkillsStore};
use store_postgres::{
    PgAgentsStore, PgApiKeysStore, PgBackend, PgDeploymentsStore, PgEnvironmentKeysStore,
    PgEnvironmentsStore, PgFilesStore, PgMemoryStore, PgSessionEventLog, PgSessionsStore,
    PgSkillVersionsStore, PgSkillsStore, PgSteps, PgVaultsStore, PgWorkStore, PgWorkspacesStore,
};
use vaults::{VaultCredentialStore, VaultStore};
use work::{WorkReads, WorkStore};

/// The entity stores.
#[derive(Clone)]
pub struct Stores {
    /// Agents.
    pub agents: Arc<dyn AgentsStore>,
    /// Environments.
    pub environments: Arc<dyn EnvironmentsStore>,
    /// Vaults.
    pub vaults: Arc<dyn VaultStore>,
    /// Vault credentials, on the same store as the vaults.
    pub vault_credentials: Arc<dyn VaultCredentialStore>,
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
    /// Session event logs as the events list reads them.
    pub session_event_list: Arc<dyn SessionEventReads>,
    /// Deployments.
    pub deployments: Arc<dyn DeploymentsStore>,
    /// Workspaces.
    pub workspaces: Arc<dyn WorkspacesStore>,
    /// Workspace API keys.
    pub api_keys: Arc<dyn ApiKeysStore>,
    /// Environment keys.
    pub environment_keys: Arc<dyn EnvironmentKeysStore>,
    /// Work items.
    pub work: Arc<dyn WorkStore>,
    /// Work items, as the edge reads them.
    pub work_reads: Arc<dyn WorkReads>,
    /// The loop's step queue.
    pub steps: Arc<dyn Steps>,
}

impl std::fmt::Debug for Stores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stores").finish_non_exhaustive()
    }
}

impl Stores {
    /// Every store on Postgres, with bytes in `blobs` and secrets in `secrets`.
    pub fn postgres(backend: PgBackend, blobs: Arc<dyn BlobStore>, secrets: Arc<dyn SecretStore>) -> Self {
        let vaults = Arc::new(PgVaultsStore::new(backend.clone(), secrets));
        let work = Arc::new(PgWorkStore::new(backend.clone()));
        let events = Arc::new(PgSessionEventLog::new(backend.clone()));
        Self {
            agents: Arc::new(PgAgentsStore::new(backend.clone())),
            environments: Arc::new(PgEnvironmentsStore::new(backend.clone())),
            vaults: vaults.clone(),
            vault_credentials: vaults,
            memory: Arc::new(PgMemoryStore::new(backend.clone(), blobs.clone())),
            files: Arc::new(PgFilesStore::new(backend.clone(), blobs.clone())),
            skills: Arc::new(PgSkillsStore::new(backend.clone(), blobs.clone())),
            skill_versions: Arc::new(PgSkillVersionsStore::new(backend.clone(), blobs)),
            sessions: Arc::new(PgSessionsStore::new(backend.clone())),
            session_events: events.clone(),
            session_event_list: events,
            deployments: Arc::new(PgDeploymentsStore::new(backend.clone())),
            workspaces: Arc::new(PgWorkspacesStore::new(backend.clone())),
            api_keys: Arc::new(PgApiKeysStore::new(backend.clone())),
            environment_keys: Arc::new(PgEnvironmentKeysStore::new(backend.clone())),
            work: work.clone(),
            work_reads: work,
            steps: Arc::new(PgSteps::new(backend)),
        }
    }

    /// The events list on `replica`, every other read and write where it was.
    #[must_use]
    pub fn reading_events_from(mut self, replica: PgBackend) -> Self {
        self.session_event_list = Arc::new(PgSessionEventLog::new(replica));
        self
    }
}
