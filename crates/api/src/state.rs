//! Application state: every resource's service built over the ports, and
//! assembled into its module.

use std::sync::Arc;

use agents::AgentsService;
use deployments::DeploymentsService;
use environments::EnvironmentsService;
use files::FilesService;
use memory::MemoriesService;
use organizations::WorkspacesService;
use sessions::SessionsService;
use shared::limits::ModelCatalog;
use shared::ports::fire::FireError;
use shared::ports::keys::{AdminKeySource, KeySource};
use shared::Module;
use skills::SkillsService;
use vaults::VaultsService;

use crate::backends::Backends;
use crate::deps::{RealDeploymentDeps, RealSessionDeps, RealSessionFiles, RealWorkspaceKeys};
use crate::egress::{EgressAdmin, SessionEgressPolicy};
use crate::sandbox_work::WorkIssuer;
use crate::telemetry::ports::Observed;
use crate::telemetry::Metrics;
use shared::hash::Signer;
use vaults::egress::VaultCredentials;
use work::{WorkService, WorkTokens};

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
    /// Signs sandbox workers' work tokens, when the agent loop runs.
    pub work_tokens: Option<WorkTokens>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").field("modules", &self.modules).finish_non_exhaustive()
    }
}

impl AppState {
    /// Build every service over `backends`, assemble the modules, and register
    /// their fire handlers with the scheduler's registry. Two modules claiming
    /// one fire kind is an error.
    pub fn build(backends: Backends) -> Result<Self, BuildError> {
        let Backends { stores, keys, admin_keys, scheduler, fires, runtime, sandbox, extra_models, metrics } =
            backends;
        let catalog = ModelCatalog::default_catalog().with(extra_models);
        let agents = Arc::new(AgentsService::new(stores.agents, catalog.clone()));
        let environments = Arc::new(EnvironmentsService::new(stores.environments));
        let sessions = SessionsService::new(
            stores.sessions,
            stores.session_events,
            Arc::new(RealSessionDeps { agents: agents.clone(), environments: environments.clone() }),
            catalog,
        );
        // One key signs work tokens and egress passwords, each under its own purpose.
        let key = sandbox.as_ref().and_then(|s| s.work_token_key.clone());
        let key = key.unwrap_or_else(|| rand::random::<[u8; 32]>().to_vec());
        let work = Arc::new(WorkService::new(WorkTokens::new(key.clone())));
        let issuer = sandbox.map(|config| {
            let egress = config.egress_admin.as_ref().map(|(url, token)| {
                Arc::new(SessionEgressPolicy::new(
                    Signer::new(key.clone()),
                    VaultCredentials::new(stores.vaults.clone()),
                    EgressAdmin::new(url, token),
                    &config.model_base_url,
                ))
            });
            Arc::new(WorkIssuer {
                work: work.clone(),
                api_url: config.api_url,
                egress,
                egress_host: config.egress_host,
                model_base_url: config.model_base_url,
            }) as Arc<dyn sessions::SandboxWork>
        });
        let sessions = Arc::new(match runtime {
            Some(runtime) => sessions.with_sandbox(runtime, issuer),
            None => sessions,
        });
        let files = Arc::new(FilesService::new(
            stores.files,
            Arc::new(RealSessionFiles { sessions: sessions.clone() }),
        ));
        let skills = Arc::new(SkillsService::new(stores.skills, stores.skill_versions));
        let deployments = Arc::new(DeploymentsService::new(
            stores.deployments,
            Arc::new(RealDeploymentDeps {
                agents: agents.clone(),
                environments: environments.clone(),
                sessions: sessions.clone(),
            }),
            scheduler,
        ));
        let modules = vec![
            agents::module(agents),
            environments::module(environments),
            vaults::module(Arc::new(VaultsService::new(stores.vaults))),
            memory::module(Arc::new(MemoriesService::new(stores.memory))),
            files::module(files),
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
        Ok(Self { keys, admin_keys, modules, metrics, work_tokens: Some(work.tokens().clone()) })
    }
}

/// The app could not be assembled.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// Two modules claim one fire kind.
    #[error(transparent)]
    Fire(#[from] FireError),
}
