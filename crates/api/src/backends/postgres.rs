//! The Postgres backends: the stores on the primary, the events list on a
//! read replica where one is set, the primary's load for the overload gate,
//! and event retention.

use std::sync::Arc;
use std::time::Duration;

use aws_common::RdsAuthToken;
use sessions::retention::Retention;
use sessions::store::DatabaseLoad;
use shared::ports::blobs::BlobStore;
use shared::ports::credentials::PasswordSource;
use shared::ports::fire::{FireRegistry, FireWorker};
use store_postgres::{
    PgBackend, PgError, PgFireClaims, PgLoad, PgRunLock, PgSessionEventLog, PgSessionsStore, PoolSettings,
    Sweep,
};

use super::Stores;
use super::feed::ChangeFeed;
use super::scheduler::Sink;
use super::secrets::Secrets;
use crate::config::{PgAuth, StoreConfig};
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

/// The pool at `store`'s primary, logging in as its `AGENTD_PG_AUTH` says,
/// without the pool's metrics: the commands' pool.
pub(crate) async fn primary(store: &StoreConfig) -> Result<PgBackend, PgError> {
    let StoreConfig::Postgres { url, auth, .. } = store;
    PgBackend::connect_as(url, PoolSettings::default(), None, login(*auth).await).await
}

/// Each new connection's password, when `auth` mints one.
async fn login(auth: PgAuth) -> Option<Arc<dyn PasswordSource>> {
    match auth {
        PgAuth::Password => None,
        PgAuth::AwsIam => Some(Arc::new(RdsAuthToken::new(aws_common::sdk_config(None, None).await))),
    }
}

/// Connect the pools; the load measure, the cleanup sweep and, with a
/// `retention` period, the retention runs start here.
pub(super) async fn connect(
    store: &StoreConfig,
    retention: Option<Duration>,
    metrics: &Arc<Metrics>,
    outside: Outside,
) -> Result<Postgres, PgError> {
    let StoreConfig::Postgres { url, read_url, pool, auth } = store;
    let (pool, password) = (*pool, login(*auth).await);
    let observer = telemetry::db::observer(metrics.clone());
    let pg = PgBackend::connect_as(url, pool, Some(observer.clone()), password.clone()).await?;
    let meter = opentelemetry::global::meter("agentd");
    telemetry::db::watch_pool(&meter, pg.diesel.clone());
    let Outside { blobs, secrets } = outside;
    let secrets = secrets.on(&pg, metrics);
    drop(Sweep::new(pg.clone(), secrets.clone(), blobs.clone()).spawn());
    let mut stores = Stores::postgres(pg.clone(), blobs, secrets);
    if let Some(read_url) = read_url {
        stores = stores
            .reading_events_from(PgBackend::connect_as(read_url, pool, Some(observer), password).await?);
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
