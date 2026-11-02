//! Application state: every entity service constructed and wired, plus the
//! API-key registry that maps keys to `(org, workspace)`.

use std::collections::HashMap;
use std::sync::Arc;

use agents::AgentsService;
use blobs_s3::BlobStore;
use deployments::DeploymentsService;
use environments::EnvironmentsService;
use files::FilesService;
use memory::MemoriesService;
use parking_lot::RwLock;
use sessions::SessionsService;
use shared::limits::ModelCatalog;
use shared::ports::SchedulerPort;
use shared::{AuthContext, OrgId, WorkspaceId};
use skills::SkillsService;
use store_postgres::{
    PgAgentsStore, PgBackend, PgDeploymentsStore, PgEnvironmentsStore, PgFilesStore, PgMemoryStore,
    PgSessionsStore, PgSkillsStore, PgVaultsStore,
};
use vaults::VaultsService;

use crate::deps::{RealDeploymentDeps, RealSessionDeps};

/// Maps API keys to their tenant. Populated from config at boot;
/// dev/test mode registers keys programmatically.
#[derive(Debug, Clone, Default)]
pub struct ApiKeyRegistry {
    inner: Arc<RwLock<HashMap<String, AuthContext>>>,
}

impl ApiKeyRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a key.
    pub fn register(
        &self,
        key: impl Into<String>,
        org: impl Into<String>,
        workspace: impl Into<String>,
        api_key_id: impl Into<String>,
    ) {
        self.inner.write().insert(
            key.into(),
            AuthContext {
                org: OrgId::new(org),
                workspace: WorkspaceId::new(workspace),
                api_key_id: api_key_id.into(),
            },
        );
    }

    /// Resolve a key to its tenant.
    pub fn resolve(&self, key: &str) -> Option<AuthContext> {
        self.inner.read().get(key).cloned()
    }
}

/// All services, wired.
#[derive(Clone)]
pub struct AppState {
    /// Key → tenant registry.
    pub keys: ApiKeyRegistry,
    /// Agents.
    pub agents: Arc<AgentsService>,
    /// Environments.
    pub environments: Arc<EnvironmentsService>,
    /// Vaults.
    pub vaults: Arc<VaultsService>,
    /// Memory.
    pub memories: Arc<MemoriesService>,
    /// Files.
    pub files: Arc<FilesService>,
    /// Skills.
    pub skills: Arc<SkillsService>,
    /// Sessions.
    pub sessions: Arc<SessionsService>,
    /// Deployments.
    pub deployments: Arc<DeploymentsService>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

impl AppState {
    /// Build the durable stack: every entity store is Postgres (records) + a
    /// shared object store (blobs/secrets). No in-memory state — replicas are
    /// interchangeable. `backend` is a live pool; `blobs` is one object store
    /// reused across files/skills/memory-content/vault-secrets (distinct key
    /// prefixes). Only the key registry and model catalog stay in memory, both
    /// loaded from central config at boot.
    pub fn from_postgres(
        backend: PgBackend,
        blobs: Arc<dyn BlobStore>,
        scheduler: Arc<dyn SchedulerPort>,
    ) -> Self {
        let keys = ApiKeyRegistry::new();
        let agents = Arc::new(AgentsService::new(
            Arc::new(PgAgentsStore::new(backend.clone())),
            ModelCatalog::default_catalog(),
        ));
        let environments =
            Arc::new(EnvironmentsService::new(Arc::new(PgEnvironmentsStore::new(backend.clone()))));
        let vaults =
            Arc::new(VaultsService::new(Arc::new(PgVaultsStore::new(backend.clone(), blobs.clone()))));
        let memories =
            Arc::new(MemoriesService::new(Arc::new(PgMemoryStore::new(backend.clone(), blobs.clone()))));
        let files = Arc::new(FilesService::new(Arc::new(PgFilesStore::new(backend.clone(), blobs.clone()))));
        let skills =
            Arc::new(SkillsService::new(Arc::new(PgSkillsStore::new(backend.clone(), blobs.clone()))));
        let sessions = Arc::new(SessionsService::new(
            Arc::new(PgSessionsStore::new(backend.clone())),
            Arc::new(RealSessionDeps { agents: agents.clone(), environments: environments.clone() }),
        ));
        let deployments = Arc::new(DeploymentsService::new(
            Arc::new(PgDeploymentsStore::new(backend)),
            Arc::new(RealDeploymentDeps {
                agents: agents.clone(),
                environments: environments.clone(),
                sessions: sessions.clone(),
            }),
            scheduler,
        ));
        Self { keys, agents, environments, vaults, memories, files, skills, sessions, deployments }
    }
}
