//! The scheduler sink `AGENTD_SCHEDULER` selects. Postgres: the built-in
//! scheduler, whose tick every replica runs. Temporal: Temporal Schedules,
//! whose actions agentd's Temporal worker writes to the fire queue; the
//! built-in tick does not run. Either way, the fire workers run the queue.

use std::sync::Arc;
use std::time::Duration;

use scheduler_temporal::TemporalScheduler;
use shared::ports::SchedulerPort;
use shared::ports::fire::FireRegistry;
use store_postgres::{PgBackend, PgScheduler};

use super::BootError;
use crate::config::SchedulerConfig;

/// How often each replica claims due schedules (cron is minute-grained).
const TICK: Duration = Duration::from_secs(10);

/// Where schedule changes go, and what turns schedules into occurrences.
#[derive(Debug, Clone)]
pub(super) enum Sink {
    Postgres(PgScheduler),
    Temporal(TemporalScheduler),
}

impl Sink {
    /// The sink `config` selects, over `pg` for the built-in one.
    pub(super) async fn connect(config: &SchedulerConfig, pg: &PgBackend) -> Result<Self, BootError> {
        match config {
            SchedulerConfig::Postgres => Ok(Self::Postgres(PgScheduler::new(pg.clone()))),
            SchedulerConfig::Temporal(temporal) => {
                let scheduler = TemporalScheduler::connect(temporal.clone()).await;
                scheduler.map(Self::Temporal).map_err(|e| BootError::Scheduler(e.to_string()))
            }
        }
    }

    /// The port the schedule outbox applies changes to.
    pub(super) fn port(&self) -> Arc<dyn SchedulerPort> {
        match self {
            Self::Postgres(tick) => Arc::new(tick.clone()),
            Self::Temporal(temporal) => Arc::new(temporal.clone()),
        }
    }

    /// Start turning schedules into occurrences of `fires`' queue; `enqueued`
    /// hears how many each built-in tick wrote.
    pub(super) fn start(self, fires: &Arc<FireRegistry>, enqueued: impl Fn(usize) + Send + 'static) {
        match self {
            Self::Postgres(tick) => drop(tick.spawn(TICK, enqueued)),
            Self::Temporal(temporal) => drop(scheduler_temporal::spawn_worker(&temporal, fires.clone())),
        }
    }
}
