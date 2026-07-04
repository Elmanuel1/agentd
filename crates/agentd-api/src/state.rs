//! Application state: every entity service constructed and wired, plus the
//! API-key registry that maps keys to `(org, workspace)`.

use std::collections::HashMap;
use std::sync::Arc;

use agentd_agents::{AgentsService, InMemoryAgentsStore};
use agentd_core::limits::ModelCatalog;
use agentd_core::ports::SchedulerPort;
use agentd_core::{AuthContext, OrgId, WorkspaceId};
use agentd_deployments::{DeploymentsService, InMemoryDeploymentsStore};
use agentd_environments::{EnvironmentsService, InMemoryEnvironmentsStore};
use agentd_files::store::InMemoryFilesStore;
use agentd_files::FilesService;
use agentd_memory::{InMemoryMemoryStore, MemoriesService};
use agentd_sessions::{InMemorySessionsStore, SessionsService};
use agentd_skills::{InMemorySkillsStore, SkillsService};
use agentd_vaults::{InMemoryVaultsStore, VaultsService};
use parking_lot::RwLock;

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
                org: OrgId(org.into()),
                workspace: WorkspaceId(workspace.into()),
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
    /// Build the full in-memory stack (dev/test; persistent stores slot in
    /// behind the same traits).
    pub fn in_memory(scheduler: Arc<dyn SchedulerPort>) -> Self {
        let keys = ApiKeyRegistry::new();
        let agents = Arc::new(AgentsService::new(
            Arc::new(InMemoryAgentsStore::new()),
            ModelCatalog::default_catalog(),
        ));
        let environments = Arc::new(EnvironmentsService::new(Arc::new(
            InMemoryEnvironmentsStore::new(),
        )));
        let vaults = Arc::new(VaultsService::new(Arc::new(InMemoryVaultsStore::new())));
        let memories = Arc::new(MemoriesService::new(Arc::new(InMemoryMemoryStore::new())));
        let files = Arc::new(FilesService::new(Arc::new(InMemoryFilesStore::new())));
        let skills = Arc::new(SkillsService::new(Arc::new(InMemorySkillsStore::new())));
        let sessions = Arc::new(SessionsService::new(
            Arc::new(InMemorySessionsStore::new()),
            Arc::new(RealSessionDeps {
                agents: agents.clone(),
                environments: environments.clone(),
            }),
        ));
        let deployments = Arc::new(DeploymentsService::new(
            Arc::new(InMemoryDeploymentsStore::new()),
            Arc::new(RealDeploymentDeps {
                agents: agents.clone(),
                environments: environments.clone(),
                sessions: sessions.clone(),
            }),
            scheduler,
        ));
        Self {
            keys,
            agents,
            environments,
            vaults,
            memories,
            files,
            skills,
            sessions,
            deployments,
        }
    }
}
