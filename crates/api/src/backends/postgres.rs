//! The Postgres backends: the stores on the primary, the events list on a
//! read replica where one is set, the primary's load for the overload gate,
//! and event retention.

use std::sync::Arc;
use std::time::Duration;

use sessions::retention::Retention;
use sessions::store::DatabaseLoad;
use shared::ports::blobs::BlobStore;
use shared::ports::fire::{FireRegistry, FireWorker};
use store_postgres::{
    PgBackend, PgError, PgFireClaims, PgLoad, PgRunLock, PgSessionEventLog, PgSessionsStore, PoolSettings,
};

use super::Stores;
use super::feed::ChangeFeed;
use super::scheduler::Sink;
use super::secrets::Secrets;
use crate::telemetry::scheduler::{self, SchedulerMetrics};
use crate::telemetry::{self, Metrics};

/// How often each replica runs the due fires of the fire queue.
const FIRES: Duration = Duration::from_secs(1);

/// What the Postgres backends give the rest of the app.
pub(super) struct Postgres {
    pub(super) stores: Stores,
    pub(super) load: Arc<dyn DatabaseLoad>,
    /// The primary, for the jobs that run on it.
    pub(super) backend: PgBackend,
}

/// What the stores keep outside Postgres: bytes, and secrets unless the
/// Postgres store keeps them.
pub(super) struct Outside {
    pub(super) blobs: Arc<dyn BlobStore>,
    pub(super) secrets: Secrets,
}

/// Connect the pools; the load measure and, with a `retention` period, the
/// retention runs start here.
pub(super) async fn connect(
    url: &str,
    read_url: Option<&str>,
    pool: PoolSettings,
    retention: Option<Duration>,
    metrics: &Arc<Metrics>,
    outside: Outside,
) -> Result<Postgres, PgError> {
    let observer = telemetry::db::observer(metrics.clone());
    let pg = PgBackend::connect_observed(url, pool, observer.clone()).await?;
    let meter = opentelemetry::global::meter("agentd");
    telemetry::db::watch_pool(&meter, pg.diesel.clone());
    let Outside { blobs, secrets } = outside;
    let mut stores = Stores::postgres(pg.clone(), blobs, secrets.on(&pg, metrics));
    if let Some(read_url) = read_url {
        stores = stores.reading_events_from(PgBackend::connect_observed(read_url, pool, observer).await?);
    }
    let load = PgLoad::new(pg.diesel.clone());
    drop(load.clone().spawn());
    drop(telemetry::steps::watch(&meter, stores.steps.clone()));
    if let Some(period) = retention {
        let last_success = telemetry::retention::last_success(&meter);
        let retention = Retention::new(
            Box::new(PgSessionsStore::new(pg.clone())),
            Box::new(PgSessionEventLog::new(pg.clone())),
            Box::new(PgRunLock::new(pg.clone())),
            period,
        );
        drop(retention.spawn(move |at| last_success.record(at)));
    }
    Ok(Postgres { stores, load: Arc::new(load), backend: pg })
}

/// What every replica runs: the scheduler sink's side that writes each
/// occurrence to the fire queue (the built-in tick, or the Temporal worker),
/// the change feed's consumer of an external scheduler's fires, which writes
/// them there too, and the fire workers, which run the queue. They start once
/// the handlers are registered.
#[derive(Debug)]
pub struct FireSources {
    sink: Sink,
    feed: ChangeFeed,
    queue: PgFireClaims,
    metrics: SchedulerMetrics,
}

impl FireSources {
    pub(super) fn new(sink: Sink, feed: ChangeFeed, queue: PgFireClaims, metrics: SchedulerMetrics) -> Self {
        Self { sink, feed, queue, metrics }
    }

    /// Start, running the queue through `fires`.
    pub(crate) fn start(self, fires: &Arc<FireRegistry>) {
        let (ticked, ended) = (self.metrics.clone(), self.metrics);
        self.sink.start(fires, move |n| ticked.enqueued(n));
        drop(self.feed.spawn_fires(fires.clone()));
        drop(FireWorker::new(fires.clone(), move |ending, lag| ended.ended(ending, lag)).spawn(FIRES));
        drop(scheduler::watch_due(&opentelemetry::global::meter("agentd"), self.queue.clone()));
        drop(self.queue.spawn_sweep());
    }
}

/// The fire registry over `queue`, whose writes count into the scheduler's metrics, and those metrics.
pub(super) fn fire_registry(queue: &PgFireClaims) -> (Arc<FireRegistry>, SchedulerMetrics) {
    let metrics = SchedulerMetrics::new(&opentelemetry::global::meter("agentd"));
    let delivered = metrics.clone();
    let registry =
        FireRegistry::new(Arc::new(queue.clone())).counting_enqueued(move |n| delivered.enqueued(n));
    (Arc::new(registry), metrics)
}
