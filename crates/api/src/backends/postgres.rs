//! The Postgres backends: the stores on the primary, the events list on a
//! read replica where one is set, the primary's load for the overload gate,
//! and event retention.

use std::sync::Arc;
use std::time::Duration;

use sessions::retention::Retention;
use sessions::store::DatabaseLoad;
use shared::ports::blobs::BlobStore;
use shared::ports::secrets::SecretStore;
use store_postgres::{
    PgBackend, PgError, PgLoad, PgRunLock, PgSessionEventLog, PgSessionsStore, PoolSettings,
};

use super::Stores;
use crate::telemetry::{self, Metrics};

/// What the Postgres backends give the rest of the app.
pub(super) struct Postgres {
    pub(super) stores: Stores,
    pub(super) load: Arc<dyn DatabaseLoad>,
}

/// Connect the pools; the load measure and, with a `retention` period, the
/// retention runs start here.
pub(super) async fn connect(
    url: &str,
    read_url: Option<&str>,
    pool: PoolSettings,
    retention: Option<Duration>,
    metrics: &Arc<Metrics>,
    blobs: Arc<dyn BlobStore>,
    secrets: Arc<dyn SecretStore>,
) -> Result<Postgres, PgError> {
    let observer = telemetry::db::observer(metrics.clone());
    let pg = PgBackend::connect_observed(url, pool, observer.clone()).await?;
    let meter = opentelemetry::global::meter("agentd");
    telemetry::db::watch_pool(&meter, pg.diesel.clone());
    let mut stores = Stores::postgres(pg.clone(), blobs, secrets);
    if let Some(read_url) = read_url {
        stores = stores.reading_events_from(PgBackend::connect_observed(read_url, pool, observer).await?);
    }
    let load = PgLoad::new(pg.diesel.clone());
    drop(load.clone().spawn());
    if let Some(period) = retention {
        let last_success = telemetry::retention::last_success(&meter);
        let retention = Retention::new(
            Box::new(PgSessionsStore::new(pg.clone())),
            Box::new(PgSessionEventLog::new(pg.clone())),
            Box::new(PgRunLock::new(pg)),
            period,
        );
        drop(retention.spawn(move |at| last_success.record(at)));
    }
    Ok(Postgres { stores, load: Arc::new(load) })
}
