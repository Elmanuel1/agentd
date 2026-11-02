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

use crate::backends::Backends;
use crate::deps::{RealDeploymentDeps, RealSessionDeps};

/// The assembled app: the key source and every resource's module.
pub struct AppState {
    /// API-key resolution.
    pub keys: Arc<dyn KeySource>,
    /// One module per resource.
    pub modules: Vec<Module>,
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
    pub fn build(backends: Backends) -> Result<Self, FireError> {
        let Backends { stores, keys, scheduler, fires, runtime } = backends;
        let agents = Arc::new(AgentsService::new(stores.agents, ModelCatalog::default_catalog()));
        let environments = Arc::new(EnvironmentsService::new(stores.environments));
        let sessions = SessionsService::new(
            stores.sessions,
            stores.session_events,
            Arc::new(RealSessionDeps { agents: agents.clone(), environments: environments.clone() }),
            ModelCatalog::default_catalog(),
        );
        let sessions = Arc::new(match runtime {
            Some(runtime) => sessions.with_sandbox(runtime),
            None => sessions,
        });
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
            files::module(Arc::new(FilesService::new(stores.files))),
            skills::module(Arc::new(SkillsService::new(stores.skills, stores.skill_versions))),
            sessions::module(sessions),
            deployments::module(deployments),
        ];
        for handler in modules.iter().flat_map(|m| &m.fire_handlers) {
            fires.register(handler.clone())?;
        }
        Ok(Self { keys, modules })
    }
}
