//! The loop workers of a `loop` or `all` replica: the model providers whose
//! keys are stored, the account-wide budget, and the worker pool.

use std::sync::Arc;
use std::time::Duration;

use engine::model::budget::{ModelBudget, Unlimited};
use engine::model::retry::Retry;
use engine::model::{ModelRouter, ProviderConfig};
use engine::worker::{Loop, Models, Settings, Stores as LoopStores};
use sessions::signals::Signals;
use shared::ports::secrets::SecretStore;
use signals_valkey::ValkeyBudget;

use work::{WorkService, WorkTokens};

use crate::backends::{BootError, Stores, SystemKeys};
use crate::config::Config;
use crate::model_key::{self, ANTHROPIC, OPENROUTER};

/// A claim's lease outlasts the step's model deadline by this much.
const LEASE_MARGIN: Duration = Duration::from_secs(120);

/// Start the loop workers `config` sets, over `stores`.
pub async fn spawn(
    config: &Config,
    stores: &Stores,
    signals: Arc<dyn Signals>,
    secrets: &dyn SecretStore,
    system: &SystemKeys,
) -> Result<tokio::task::JoinHandle<()>, BootError> {
    let c = &config.agent_loop;
    let key = |e: String| BootError::Loop(format!("model key: {e}"));
    let anthropic = model_key::read(secrets, ANTHROPIC).await.map_err(key)?;
    let openrouter = model_key::read(secrets, OPENROUTER).await.map_err(key)?;
    let providers = ProviderConfig {
        anthropic: anthropic.map(|k| (k, c.anthropic_base_url.clone())),
        openai_compat: openrouter.map(|k| (k, config.sandbox.model_base_url.clone())),
    };
    let client = ModelRouter::from_config(&providers).map_err(BootError::Loop)?;
    let budget: Arc<dyn ModelBudget> = match (&config.streams.valkey_url, c.rpm.or(c.itpm)) {
        (Some(url), Some(_)) => {
            Arc::new(ValkeyBudget::connect(url, c.rpm, c.itpm).map_err(|e| BootError::Loop(e.to_string()))?)
        }
        _ => Arc::new(Unlimited),
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
            WorkTokens::new(system.work_token_key.clone()),
            system.api_base_url.clone(),
        )),
    };
    let settings = Settings {
        owner: format!("loop-{}", uuid::Uuid::new_v4()),
        lease: c.deadline + LEASE_MARGIN,
        per_workspace: c.per_workspace,
        steps: c.steps,
    };
    Ok(Loop::new(loop_stores, models, signals, settings).spawn())
}
