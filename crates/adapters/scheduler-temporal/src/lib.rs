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

/// The schedule's cron as Temporal's cron strings. POSIX fires on either day
/// field when both are restricted, and Temporal requires both, so that case
/// is the union of one string per day field.
fn crons(cron: &str) -> Vec<String> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    let [minute, hour, dom, month, dow] = fields.as_slice() else {
        return vec![cron.to_owned()];
    };
    if *dom == "*" || *dow == "*" {
        return vec![cron.to_owned()];
    }
    vec![format!("{minute} {hour} {dom} {month} *"), format!("{minute} {hour} * {month} {dow}")]
}

fn timer(spec: &ScheduleSpec) -> TemporalSpec {
    TemporalSpec::builder().cron_strings(crons(&spec.cron)).timezone_name(spec.timezone.clone()).build()
}

/// Occurrences worked out per step while the catch-up window is sized.
const WINDOW_CHUNK: usize = 1000;

/// The longest gap between the schedule's occurrences over the next five
/// years (the span a cron's day patterns repeat within, Feb 29 included), and
/// the gap past it, at least Temporal's 10 s minimum: Temporal measures the window from a
/// missed time, so every occurrence stays deliverable until its following one
/// is due, and the fire queue skips any delivered after that.
fn catch_up_window(spec: &ScheduleSpec, now: chrono::DateTime<chrono::Utc>) -> std::time::Duration {
    let floor = std::time::Duration::from_secs(10);
    let Ok(compiled) = spec.compile() else { return floor };
    let (mut longest, mut last) = (chrono::Duration::zero(), None);
    let end = now + chrono::Duration::days(366 * 5);
    let mut from = now;
    while last.is_none_or(|t| t < end) {
        let chunk = compiled.next_fires(from, WINDOW_CHUNK);
        let Some(&tail) = chunk.last() else { break };
        for t in chunk {
            longest = longest.max(last.map_or(chrono::Duration::zero(), |previous| t - previous));
            last = Some(t);
        }
        from = tail;
    }
    longest.to_std().unwrap_or_default().max(floor)
}

/// A create that fails is never [`SchedulerError::Unknown`], which the outbox reads as applied: a missing
/// namespace answers `NOT_FOUND` too.
fn not_created(target: &FireTarget, e: ScheduleError) -> SchedulerError {
    match refused(target, e) {
        SchedulerError::Unknown(key) => SchedulerError::Unavailable(format!("{key}: not found at create")),
        other => other,
    }
}

#[async_trait]
impl SchedulerPort for TemporalScheduler {
    async fn upsert(&self, spec: ScheduleSpec) -> Result<(), SchedulerError> {
        spec.compile().map_err(|e| SchedulerError::InvalidSpec(e.to_string()))?;
        let handle = self.client.get_schedule_handle(spec.target.key());
        let (timer, action, window) =
            (timer(&spec), self.action(&spec), catch_up_window(&spec, chrono::Utc::now()));
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
        let created = created.map_err(|e| not_created(&spec.target, e))?;
        created
            .update(move |u| _ = u.set_catchup_window(window), RpcOptions::default())
            .await
            .map_err(|e| not_created(&spec.target, e))
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

#[cfg(test)]
mod tests;
