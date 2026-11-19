//! agentd's Temporal worker: [`FireWorkflow`], which each schedule action
//! starts, runs one activity that writes the action's occurrence to the fire
//! queue. Temporal retries the activity until the write commits, so a
//! delivery is never lost; the queue's key makes a repeat write nothing.

// `#[workflow_methods]` generates a type per method, with no `Debug`.
#![allow(missing_debug_implementations)]

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shared::ports::ScheduleSpec;
use shared::ports::fire::FireRegistry;
use temporalio_common::search_attributes::{SearchAttributeKey, Timestamp};
use temporalio_macros::{activities, workflow, workflow_methods};
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use temporalio_sdk::{
    ActivityOptions, ApplicationFailure, Runtime, Worker, WorkerOptions, WorkflowContext, WorkflowResult,
};

use crate::TemporalScheduler;

/// The time a schedule meant an action to start, which Temporal sets on every workflow a schedule starts.
const SCHEDULED_AT: SearchAttributeKey<Timestamp> =
    SearchAttributeKey::datetime("TemporalScheduledStartTime");

/// One occurrence a schedule action delivers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Occurrence {
    /// The schedule.
    pub spec: ScheduleSpec,
    /// Its listed time.
    pub scheduled_at: DateTime<Utc>,
}

/// Started by each action of a schedule, with the schedule's spec.
#[workflow]
#[derive(Debug, Default)]
pub struct FireWorkflow;

#[workflow_methods]
impl FireWorkflow {
    /// Write the action's occurrence to agentd's fire queue.
    #[run]
    pub async fn run(ctx: &mut WorkflowContext<Self>, spec: ScheduleSpec) -> WorkflowResult<()> {
        let scheduled = ctx.search_attributes().get(&SCHEDULED_AT).map(std::time::SystemTime::try_from);
        let Some(Ok(scheduled)) = scheduled else {
            return Err(
                ApplicationFailure::non_retryable("the workflow was not started by a schedule").into()
            );
        };
        let occurrence = Occurrence { spec, scheduled_at: DateTime::<Utc>::from(scheduled) };
        let options = ActivityOptions::start_to_close_timeout(Duration::from_secs(30));
        ctx.execute_activity(Enqueue::enqueue, occurrence, options).await?;
        Ok(())
    }
}

/// The activity, over the fire queue of `registry`.
#[derive(Debug)]
pub struct Enqueue {
    registry: Arc<FireRegistry>,
}

#[activities]
impl Enqueue {
    /// Write `occurrence` to the fire queue.
    #[activity]
    pub async fn enqueue(
        self: Arc<Self>,
        _ctx: ActivityContext,
        occurrence: Occurrence,
    ) -> Result<(), ActivityError> {
        self.registry.enqueue(&occurrence.spec, occurrence.scheduled_at).await?;
        Ok(())
    }
}

/// Run agentd's Temporal worker for `scheduler`'s task queue, writing occurrences through `registry`.
pub fn spawn_worker(
    scheduler: &TemporalScheduler,
    registry: Arc<FireRegistry>,
) -> std::thread::JoinHandle<()> {
    let (client, task_queue) = (scheduler.client().clone(), scheduler.task_queue().to_owned());
    // The worker's workflows are not Send, so it runs on a thread of its own.
    std::thread::spawn(move || {
        let Ok(tokio) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return tracing::error!("scheduler: the Temporal worker's runtime cannot start");
        };
        tokio.block_on(async move {
            loop {
                if let Err(e) = run(&client, &task_queue, registry.clone()).await {
                    tracing::error!(error = %e, "scheduler: the Temporal worker stopped; starting again");
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    })
}

async fn run(
    client: &temporalio_client::Client,
    task_queue: &str,
    registry: Arc<FireRegistry>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let runtime = Runtime::from_current_tokio(Default::default())?;
    let options = WorkerOptions::new(task_queue)
        .register_workflow::<FireWorkflow>()?
        .register_activities(Enqueue { registry })
        .build();
    Worker::new(&runtime, client.clone(), options)?.run().await?;
    Ok(())
}
