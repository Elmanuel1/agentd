//! A loop worker: up to `steps` steps at once, each claimed from the queue,
//! run and committed by itself. The worker holds a database connection only
//! while it claims, reads or writes, and nothing between steps.

use std::sync::Arc;
use std::time::Duration;

use sessions::signals::{Signal, Signals};
use sessions::steps::Steps;
use sessions::store::{SessionEventLog, SessionsStore};
use tokio::sync::Semaphore;
use work::WorkStore;

use crate::model::budget::ModelBudget;
use crate::model::retry::Retry;
use crate::model::ModelClient;

mod reply;
mod step;
mod take;
mod watch;

/// How long an idle worker waits for a signal before it looks again.
const IDLE: Duration = Duration::from_secs(1);

/// The stores a step reads and writes.
#[derive(Clone)]
pub struct Stores {
    /// The step queue.
    pub steps: Arc<dyn Steps>,
    /// The session rows.
    pub sessions: Arc<dyn SessionsStore>,
    /// The event log.
    pub log: Arc<dyn SessionEventLog>,
    /// The work items, where the session's worker runs its built-in calls.
    pub work: Arc<dyn WorkStore>,
}

/// The model a step calls.
#[derive(Clone)]
pub struct Models {
    /// The provider clients.
    pub client: Arc<dyn ModelClient>,
    /// How a call is retried.
    pub retry: Retry,
    /// The account-wide budget.
    pub budget: Arc<dyn ModelBudget>,
    /// The output cap per call (`AGENTD_MAX_OUTPUT_TOKENS`).
    pub max_tokens: u32,
}

impl std::fmt::Debug for Stores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stores").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Models {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Models")
            .field("retry", &self.retry)
            .field("max_tokens", &self.max_tokens)
            .finish_non_exhaustive()
    }
}

/// How the worker claims.
#[derive(Debug, Clone)]
pub struct Settings {
    /// This worker's name on the leases it takes.
    pub owner: String,
    /// A claim's lease, longer than the step's model deadline.
    pub lease: Duration,
    /// Steps held at once per workspace (`AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE`).
    pub per_workspace: i64,
    /// Steps this worker runs at once (`AGENTD_STEPS_PER_WORKER`).
    pub steps: usize,
}

/// The loop.
#[derive(Clone)]
pub struct Loop {
    stores: Stores,
    models: Models,
    signals: Arc<dyn Signals>,
    settings: Settings,
}

impl std::fmt::Debug for Loop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loop").field("settings", &self.settings).finish_non_exhaustive()
    }
}

impl Loop {
    /// A loop over `stores` calling `models`, telling every replica's
    /// streams through `signals`.
    pub fn new(stores: Stores, models: Models, signals: Arc<dyn Signals>, settings: Settings) -> Self {
        Self { stores, models, signals, settings }
    }

    /// Claim and run steps until the task is aborted.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        let me = Arc::new(self);
        tokio::spawn(async move {
            let permits = Arc::new(Semaphore::new(me.settings.steps));
            let mut woken = me.signals.subscribe();
            loop {
                let Ok(permit) = permits.clone().acquire_owned().await else { return };
                let lease = i64::try_from(me.settings.lease.as_secs()).unwrap_or(i64::MAX);
                let claimed =
                    me.stores.steps.claim(&me.settings.owner, lease, me.settings.per_workspace).await;
                let claim = match claimed {
                    Ok(Some(claim)) => claim,
                    Ok(None) => {
                        drop(permit);
                        let _ = tokio::time::timeout(IDLE, woken.recv()).await;
                        continue;
                    }
                    Err(e) => {
                        drop(permit);
                        tracing::warn!(error = %e, "loop: could not claim a step");
                        tokio::time::sleep(IDLE).await;
                        continue;
                    }
                };
                let worker = me.clone();
                tokio::spawn(async move {
                    worker.step(claim).await;
                    drop(permit);
                });
            }
        })
    }

    /// Tell every replica's streams the session changed.
    async fn publish(&self, signal: &Signal) {
        if let Err(e) = self.signals.publish(signal).await {
            tracing::warn!(session = %signal.session, error = %e, "loop: could not signal the session's streams");
        }
    }
}
