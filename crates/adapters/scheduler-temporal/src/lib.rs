//! scheduler-temporal: [`SchedulerPort`] over Temporal Schedules, selected by
//! `AGENTD_SCHEDULER=temporal`. Temporal holds the timers; each schedule's
//! action starts [`FireWorkflow`] on agentd's task queue, whose one activity
//! writes the occurrence to agentd's fire queue, where agentd's workers run
//! it as they run every fire. A schedule carries no Temporal jitter: the fire
//! queue applies live's. Its overlap policy is `AllowAll`, since a fire only
//! writes a row, and its catch-up window reaches its following occurrence, as
//! the fire queue's does.

use async_trait::async_trait;
use shared::ports::{FireTarget, ScheduleSpec, ScheduleState, SchedulerError, SchedulerPort};
use temporalio_client::schedules::{
    CreateScheduleOptions, DeleteScheduleOptions, PauseScheduleOptions, ScheduleAction, ScheduleError,
    ScheduleOverlapPolicy, ScheduleSpec as TemporalSpec, UnpauseScheduleOptions,
};
use temporalio_client::tonic::Code;
use temporalio_client::{Client, ClientOptions, Connection, ConnectionOptions, RpcOptions, Url};

mod workflow;
pub use workflow::{FireWorkflow, Occurrence, spawn_worker};

/// Where agentd reaches Temporal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalConfig {
    /// The frontend, `http://host:7233`.
    pub address: String,
    /// The namespace the schedules live in.
    pub namespace: String,
    /// The task queue agentd's worker polls.
    pub task_queue: String,
}

/// [`SchedulerPort`] over Temporal Schedules.
#[derive(Clone)]
pub struct TemporalScheduler {
    client: Client,
    config: TemporalConfig,
}

impl std::fmt::Debug for TemporalScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TemporalScheduler").field("config", &self.config).finish_non_exhaustive()
    }
}

fn unavailable(e: impl std::fmt::Display) -> SchedulerError {
    SchedulerError::Unavailable(e.to_string())
}

/// A missing schedule is [`SchedulerError::Unknown`], a refused one is
/// [`SchedulerError::InvalidSpec`], anything else is unavailable.
fn refused(target: &FireTarget, e: ScheduleError) -> SchedulerError {
    match &e {
        ScheduleError::Rpc(status) if status.code() == Code::NotFound => {
            SchedulerError::Unknown(target.key())
        }
        ScheduleError::Rpc(status) if status.code() == Code::InvalidArgument => {
            SchedulerError::InvalidSpec(status.message().to_owned())
        }
        _ => unavailable(e),
    }
}

impl TemporalScheduler {
    /// Connect to the Temporal `config` names.
    pub async fn connect(config: TemporalConfig) -> Result<Self, SchedulerError> {
        let url = Url::parse(&config.address).map_err(unavailable)?;
        let connection =
            Connection::connect(ConnectionOptions::new(url).build()).await.map_err(unavailable)?;
        let client = Client::new(connection, ClientOptions::new(config.namespace.clone()).build())
            .map_err(unavailable)?;
        Ok(Self { client, config })
    }

    /// The client, for agentd's worker.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The task queue agentd's worker polls.
    pub fn task_queue(&self) -> &str {
        &self.config.task_queue
    }

    fn action(&self, spec: &ScheduleSpec) -> ScheduleAction {
        let workflow_id = format!("agentd-fire-{}", spec.target.key());
        ScheduleAction::start_workflow(
            FireWorkflow::run,
            spec.clone(),
            self.config.task_queue.clone(),
            workflow_id,
        )
    }
}

fn timer(spec: &ScheduleSpec) -> TemporalSpec {
    TemporalSpec::builder().cron_strings(vec![spec.cron.clone()]).timezone_name(spec.timezone.clone()).build()
}

/// The time from the next occurrence to the one after it, at least Temporal's 10 s minimum.
fn catch_up_window(spec: &ScheduleSpec) -> std::time::Duration {
    let fires = spec.compile().map(|c| c.next_fires(chrono::Utc::now(), 2)).unwrap_or_default();
    let window = match fires.as_slice() {
        [first, second] => (*second - *first).to_std().unwrap_or_default(),
        _ => std::time::Duration::ZERO,
    };
    window.max(std::time::Duration::from_secs(10))
}

#[async_trait]
impl SchedulerPort for TemporalScheduler {
    async fn upsert(&self, spec: ScheduleSpec) -> Result<(), SchedulerError> {
        spec.compile().map_err(|e| SchedulerError::InvalidSpec(e.to_string()))?;
        let handle = self.client.get_schedule_handle(spec.target.key());
        let (timer, action, window) = (timer(&spec), self.action(&spec), catch_up_window(&spec));
        let updated = handle
            .update(
                move |u| {
                    u.set_spec(timer).set_action(action).set_paused(false);
                    u.set_overlap_policy(ScheduleOverlapPolicy::AllowAll).set_catchup_window(window);
                },
                RpcOptions::default(),
            )
            .await;
        match updated.map_err(|e| refused(&spec.target, e)) {
            Err(SchedulerError::Unknown(_)) => {}
            other => return other,
        }
        let options = CreateScheduleOptions::builder()
            .action(self.action(&spec))
            .spec(self::timer(&spec))
            .overlap_policy(ScheduleOverlapPolicy::AllowAll)
            .build();
        let created = self.client.create_schedule(spec.target.key(), options).await;
        let created = created.map_err(|e| refused(&spec.target, e))?;
        created
            .update(move |u| _ = u.set_catchup_window(window), RpcOptions::default())
            .await
            .map_err(|e| refused(&spec.target, e))
    }

    async fn pause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let handle = self.client.get_schedule_handle(target.key());
        handle
            .pause(Some("paused by agentd"), PauseScheduleOptions::default())
            .await
            .map_err(|e| refused(target, e))
    }

    async fn unpause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let handle = self.client.get_schedule_handle(target.key());
        let unpaused = handle.unpause(Some("unpaused by agentd"), UnpauseScheduleOptions::default()).await;
        unpaused.map_err(|e| refused(target, e))
    }

    async fn delete(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let handle = self.client.get_schedule_handle(target.key());
        handle.delete(DeleteScheduleOptions::default()).await.map_err(|e| refused(target, e))
    }

    async fn describe(&self, target: &FireTarget) -> Result<ScheduleState, SchedulerError> {
        let handle = self.client.get_schedule_handle(target.key());
        let described = handle.describe(RpcOptions::default()).await.map_err(|e| refused(target, e))?;
        Ok(ScheduleState { paused: described.paused() })
    }
}
