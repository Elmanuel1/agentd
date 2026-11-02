//! Application state: every resource's service built over the ports, and
//! assembled into its module.

use std::sync::Arc;

use agents::AgentsService;
use deployments::DeploymentsService;
use environments::EnvironmentsService;
use files::FilesService;
use memory::MemoriesService;
use sessions::SessionsService;
use shared::limits::ModelCatalog;
use shared::ports::fire::FireError;
use shared::ports::keys::KeySource;
use shared::Module;
use skills::SkillsService;
use vaults::VaultsService;

use crate::assets::RealSessionAssets;
use crate::backends::{AgentBackends, Backends};
use crate::deps::{RealDeploymentDeps, RealSessionDeps};
use crate::egress::SessionEgressPolicy;
use crate::sandbox_work::WorkIssuer;
use crate::telemetry::ports::Observed;
use crate::telemetry::Metrics;
use engine::{Dispatcher, Engine, ModelRouter};
use shared::hash::Signer;
use vaults::egress::VaultSecrets;
use work::{RemoteSandbox, WorkService, WorkTokens};

/// The assembled app: the key source and every resource's module.
pub struct AppState {
    /// API-key resolution.
    pub keys: Arc<dyn KeySource>,
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
        let Backends { stores, keys, scheduler, fires, runtime, agent, extra_models, metrics } = backends;
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
        let key = agent.as_ref().and_then(|a| a.config.work_token_key.clone());
        let key = key.unwrap_or_else(|| rand::random::<[u8; 32]>().to_vec());
        let work = Arc::new(WorkService::new(WorkTokens::new(key.clone())));
        let egress_slot = Arc::new(std::sync::OnceLock::new());
        let issuer = agent.as_ref().map(|a| {
            let api_url = a.config.sandbox_api_url.clone();
            Arc::new(WorkIssuer { work: work.clone(), api_url, egress: egress_slot.clone() })
                as Arc<dyn sessions::SandboxWork>
        });
        let sessions = Arc::new(match runtime {
            Some(runtime) => sessions.with_sandbox(runtime, issuer),
            None => sessions,
        });
        let files = Arc::new(FilesService::new(stores.files));
        let skills = Arc::new(SkillsService::new(stores.skills, stores.skill_versions));
        if let Some(mut agent) = agent {
            let policy = Arc::new(SessionEgressPolicy::new(
                Signer::new(key),
                sessions.clone(),
                environments.clone(),
                VaultSecrets::new(stores.vaults.clone()),
                agent.config.egress_internal_hosts.clone(),
            ));
            let _ = egress_slot.set(policy.clone());
            if let Some(listener) = agent.egress.take() {
                tokio::spawn(egress_proxy::serve(listener, policy.clone()));
            }
            let assets = RealSessionAssets {
                files: files.clone(),
                skills: skills.clone(),
                environments: environments.clone(),
                egress: policy,
                egress_host: agent.config.sandbox_egress_host.clone(),
            };
            let sandbox = Arc::new(RemoteSandbox::new(work.board()));
            sessions.set_trigger(Arc::new(dispatcher(agent, sessions.clone(), assets, sandbox)?));
        }
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
        ];
        for handler in modules.iter().flat_map(|m| &m.fire_handlers) {
            fires.register(Arc::new(Observed::new(handler.clone(), metrics.clone())))?;
        }
        Ok(Self { keys, modules, metrics, work_tokens: Some(work.tokens().clone()) })
    }
}

/// The app could not be assembled.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// Two modules claim one fire kind.
    #[error(transparent)]
    Fire(#[from] FireError),
    /// The model providers could not be set up.
    #[error("models: {0}")]
    Models(String),
}

/// The agent loop over `sessions`, started by each session's client events.
fn dispatcher(
    agent: AgentBackends,
    sessions: Arc<SessionsService>,
    assets: RealSessionAssets,
    sandbox: Arc<RemoteSandbox>,
) -> Result<Dispatcher, BuildError> {
    let models = ModelRouter::from_config(&agent.config.providers).map_err(BuildError::Models)?;
    let engine = Engine {
        contract: sessions,
        models: Arc::new(models),
        sandbox,
        assets: Arc::new(assets),
        http: engine::tools::web::client(),
        search_key: agent.config.search_key.clone(),
        max_tokens: agent.config.max_tokens,
    };
    Ok(Dispatcher::new(Arc::new(engine), agent.leases))
}
