//! The Postgres backends: the stores on the primary, the events list on a
//! read replica where one is set, the primary's load for the overload gate,
//! and event retention.

use std::sync::Arc;
use std::time::Duration;

use sessions::retention::Retention;
use sessions::store::DatabaseLoad;
use shared::ports::blobs::BlobStore;
use shared::ports::fire::FireRegistry;
use store_postgres::{
    PgBackend, PgError, PgLoad, PgRunLock, PgScheduler, PgSessionEventLog, PgSessionsStore, PoolSettings,
};

use super::feed::ChangeFeed;
use super::secrets::Secrets;
use super::Stores;
use crate::telemetry::{self, Metrics};

/// How often each replica claims due schedules (cron is minute-grained).
const TICK: Duration = Duration::from_secs(10);

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

/// The fires every replica delivers: the built-in scheduler's tick (a due
/// schedule goes to whichever replica claims it first) and the fires
/// an external scheduler writes to the change feed. They start once the handlers are registered.
#[derive(Debug)]
pub struct FireSources {
    tick: PgScheduler,
    feed: ChangeFeed,
}

impl FireSources {
    pub(super) fn new(tick: PgScheduler, feed: ChangeFeed) -> Self {
        Self { tick, feed }
    }

    /// Start delivering to `fires`.
    pub(crate) fn start(self, fires: &Arc<FireRegistry>) {
        drop(self.tick.spawn(TICK));
        drop(self.feed.spawn_fires(fires.clone()));
    }
}
