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
use shared::limits::ModelCatalog;
use shared::ports::fire::FireError;
use shared::ports::keys::{AdminKeySource, EnvironmentKeySource, KeySource};
use shared::Module;
use skills::SkillsService;
use vaults::VaultsService;

use crate::backends::Backends;
use crate::deps::{RealDeploymentDeps, RealSessionDeps, RealWorkspaceKeys};
use crate::edge::scoped::WorkAccess;
use crate::egress::{EgressAdmin, SessionEgressPolicy};
use crate::harness::{self, Harness};
use crate::sandbox_work::WorkIssuer;
use crate::session_grants::SessionReach;
use crate::telemetry::ports::Observed;
use crate::telemetry::Metrics;
use shared::hash::Signer;
use vaults::egress::VaultCredentials;
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
            scheduler,
            fires,
            runtime,
            sandbox,
            extra_models,
            metrics,
            signals,
            load,
            on_loop,
            role,
        } = backends;
        let catalog = ModelCatalog::default_catalog().with(extra_models);
        let agents = Arc::new(AgentsService::new(stores.agents, catalog.clone()));
        let environments = Arc::new(EnvironmentsService::new(stores.environments));
        let reach = Arc::new(SessionReach {
            sessions: stores.sessions.clone(),
            vaults: VaultCredentials::new(stores.vault_credentials.clone()),
        });
        let sessions = SessionsService::new(
            stores.sessions,
            stores.session_events,
            Arc::new(RealSessionDeps { agents: agents.clone(), environments: environments.clone() }),
            catalog,
        )
        .with_streams(Streams::new(signals, load))
        .with_loop(on_loop)
        .with_list_reads(stores.session_event_list);
        // One key signs work tokens and egress passwords, each under its own purpose.
        let key = system.work_token_key.clone();
        let cloud =
            CloudVariables { source: reach.clone(), placeholders: Placeholders::new(system.egress_key) };
        let work = Arc::new(
            WorkService::new(stores.work.clone(), WorkTokens::new(key.clone()), system.api_base_url)
                .with_cloud_variables(cloud),
        );
        let issuer = sandbox.map(|config| {
            let egress = config.egress_admin.as_ref().map(|(url, token)| {
                Arc::new(SessionEgressPolicy::new(
                    Signer::new(key.clone()),
                    VaultCredentials::new(stores.vault_credentials.clone()),
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
        let files = Arc::new(FilesService::new(stores.files));
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
            vaults::module(Arc::new(VaultsService::new(stores.vaults, stores.vault_credentials))),
            memory::module(Arc::new(MemoriesService::new(stores.memory))),
            files::module(files.clone(), reach.clone()),
            skills::module(skills),
            harness::module(Harness { sessions: sessions.clone(), files: files.clone() }),
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
