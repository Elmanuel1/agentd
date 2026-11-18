//! Application state: every resource's service built over the ports, and
//! assembled into its module.

use std::sync::Arc;

use agents::AgentsService;
use deployments::DeploymentsService;
use environments::EnvironmentsService;
use files::FilesService;
use memory::MemoriesService;
use organizations::WorkspacesService;
use sessions::{SessionsService, Streams};
use shared::Module;
use shared::limits::ModelCatalog;
use shared::ports::fire::FireError;
use shared::ports::keys::{AdminKeySource, EnvironmentKeySource, KeySource};
use skills::SkillsService;
use vaults::{McpReach, VaultsService};

use crate::backends::Backends;
use crate::deps::{RealDeploymentDeps, RealSessionDeps, RealWorkspaceKeys};
use crate::edge::scoped::WorkAccess;
use crate::session_grants::SessionReach;
use crate::session_work::WorkQueue;
use crate::telemetry::Metrics;
use crate::telemetry::ports::Observed;
use vaults::variables::VaultCredentials;
use work::{CloudVariables, Placeholders, WorkService, WorkTokens};

/// The assembled app: the key source and every resource's module.
pub struct AppState {
    /// Workspace key resolution.
    pub keys: Arc<dyn KeySource>,
    /// Admin key resolution.
    pub admin_keys: Arc<dyn AdminKeySource>,
    /// One module per resource.
    pub modules: Vec<Module>,
    /// The instruments the edge records requests into.
    pub metrics: Arc<Metrics>,
    /// Environment key resolution.
    pub environment_keys: Arc<dyn EnvironmentKeySource>,
    /// What work tokens reach.
    pub work: WorkAccess,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").field("modules", &self.modules).finish_non_exhaustive()
    }
}

/// How long `mcp_oauth_validate` waits for a server's `initialize`.
const MCP_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

impl AppState {
    /// Build every service over `backends`, assemble the modules, and register
    /// their fire handlers with the scheduler's registry. Two modules claiming
    /// one fire kind is an error.
    pub fn build(backends: Backends) -> Result<Self, BuildError> {
        let Backends {
            stores,
            keys,
            admin_keys,
            environment_keys,
            system,
            fires,
            fire_sources,
            extra_models,
            metrics,
            signals,
            load,
            role,
            guard,
            webhooks,
        } = backends;
        let catalog = ModelCatalog::default_catalog().with(extra_models);
        let agents = Arc::new(AgentsService::new(stores.agents, catalog.clone()));
        let environments = Arc::new(EnvironmentsService::new(stores.environments));
        let probe = Arc::new(engine::mcp::Probe::new(guard.clone(), MCP_PROBE_TIMEOUT));
        let vaults = VaultsService::new(stores.vaults, stores.vault_credentials.clone());
        let vaults = Arc::new(vaults.with_mcp(McpReach { probe, guard }).with_webhooks(webhooks.clone()));
        let reach = Arc::new(SessionReach {
            sessions: stores.sessions.clone(),
            vaults: VaultCredentials::new(stores.vault_credentials.clone()),
        });
        let sessions = SessionsService::new(
            stores.sessions,
            stores.session_events,
            Arc::new(RealSessionDeps {
                agents: agents.clone(),
                environments: environments.clone(),
                vaults: vaults.clone(),
            }),
            catalog,
        )
        .with_streams(Streams::new(signals, load))
        .with_webhooks(webhooks)
        .with_list_reads(stores.session_event_list);
        let cloud =
            CloudVariables { source: reach.clone(), placeholders: Placeholders::new(&system.placeholders) };
        let work = Arc::new(
            WorkService::new(stores.work.clone(), WorkTokens::new(&system.work_tokens), system.api_base_url)
                .with_cloud_variables(cloud),
        );
        let sessions = Arc::new(sessions.with_work(Arc::new(WorkQueue { work: work.clone() })));
        let files = Arc::new(FilesService::new(stores.files));
        let skills = Arc::new(SkillsService::new(stores.skills, stores.skill_versions));
        let deployments = Arc::new(DeploymentsService::new(
            stores.deployments,
            Arc::new(RealDeploymentDeps {
                agents: agents.clone(),
                environments: environments.clone(),
                sessions: sessions.clone(),
            }),
        ));
        let modules = vec![
            agents::module(agents),
            environments::module(environments),
            vaults::module(vaults),
            memory::module(Arc::new(MemoriesService::new(stores.memory))),
            files::module(files, reach.clone()),
            skills::module(skills),
            sessions::module(sessions),
            deployments::module(deployments),
            work::module(work.clone()),
            organizations::module(Arc::new(WorkspacesService::new(
                stores.workspaces,
                Arc::new(RealWorkspaceKeys { keys: stores.api_keys }),
            ))),
        ];
        for handler in modules.iter().flat_map(|m| &m.fire_handlers) {
            fires.register(Arc::new(Observed::new(handler.clone(), metrics.clone())))?;
        }
        if let Some(sources) = fire_sources {
            sources.start(&fires);
        }
        let modules = match role.serves_api() {
            true => modules,
            false => Vec::new(),
        };
        let work = WorkAccess { tokens: work.tokens().clone(), items: stores.work_reads, grants: reach };
        Ok(Self { keys, admin_keys, environment_keys, modules, metrics, work })
    }
}

/// The app could not be assembled.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// Two modules claim one fire kind.
    #[error(transparent)]
    Fire(#[from] FireError),
}
