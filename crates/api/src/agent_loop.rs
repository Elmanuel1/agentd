//! The loop workers of a `loop` or `all` replica: the model providers whose
//! keys are set (`AGENTD_ANTHROPIC_API_KEY`, `AGENTD_OPENROUTER_API_KEY`), the
//! account-wide budget, and the worker pool.

use std::sync::Arc;
use std::time::Duration;

use engine::model::budget::{ModelBudget, Unlimited};
use engine::model::retry::Retry;
use engine::model::{ModelRouter, ProviderConfig};
use engine::worker::{Loop, Models, Settings, Stores as LoopStores};
use sessions::signals::Signals;
use shared::ports::secrets::{ExposeSecret, SecretString};
use signals_valkey::{Valkey, ValkeyBudget};

use files::FilesService;
use work::{WorkService, WorkTokens};

use crate::backends::{BootError, Stores, SystemKeys};
use crate::config::Config;

/// A claim's lease outlasts the step's model deadline by this much.
const LEASE_MARGIN: Duration = Duration::from_secs(120);

/// Start the loop workers `config` sets, over `stores`.
pub async fn spawn(
    config: &Config,
    stores: &Stores,
    signals: Arc<dyn Signals>,
    system: &SystemKeys,
) -> Result<tokio::task::JoinHandle<()>, BootError> {
    let c = &config.agent_loop;
    let key = |k: &Option<SecretString>| k.as_ref().map(|k| k.expose_secret().to_owned());
    let providers = ProviderConfig {
        anthropic: key(&config.keys.anthropic).map(|k| (k, c.anthropic_base_url.clone())),
        openai_compat: key(&config.keys.openrouter).map(|k| (k, c.model_base_url.clone())),
    };
    let client = ModelRouter::from_config(&providers).map_err(BootError::Loop)?;
    let budget: Arc<dyn ModelBudget> = match c.rpm.or(c.itpm) {
        Some(_) => {
            let streams = &config.streams;
            let valkey = Valkey::open(&streams.valkey_url, streams.valkey_cluster);
            let budget = valkey.and_then(|v| ValkeyBudget::connect(&v, c.rpm, c.itpm));
            Arc::new(budget.map_err(|e| BootError::Loop(e.to_string()))?)
        }
        None => Arc::new(Unlimited),
    };
    let models = Models {
        client: Arc::new(client),
        retry: Retry { attempts: c.attempts, deadline: c.deadline, ..Retry::default() },
        budget,
        max_tokens: c.max_output_tokens,
    };
    let loop_stores = LoopStores {
        steps: stores.steps.clone(),
        sessions: stores.sessions.clone(),
        log: stores.session_events.clone(),
        work: Arc::new(WorkService::new(
            stores.work.clone(),
            WorkTokens::new(&system.work_tokens),
            system.api_base_url.clone(),
        )),
        files: Arc::new(FilesService::new(stores.files.clone())),
    };
    let settings = Settings {
        owner: format!("loop-{}", uuid::Uuid::new_v4()),
        lease: c.deadline + LEASE_MARGIN,
        per_workspace: c.per_workspace,
        steps: c.steps,
    };
    Ok(Loop::new(loop_stores, models, signals, settings).spawn())
}
