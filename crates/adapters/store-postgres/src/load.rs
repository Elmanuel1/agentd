//! The database's load as the overload gate reads it: the pool's slow
//! checkouts, and the database's connections against `max_connections`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use diesel::sql_types::BigInt;
use diesel_async::RunQueryDsl;
use sessions::store::{DatabaseLoad, StoreError};

use crate::{diesel_be, pool_be, DieselPgPool};

/// How far back a slow checkout counts, and how often connections are counted.
pub const WINDOW: Duration = Duration::from_secs(10);

/// The share of `max_connections` past which the database is overloaded, in percent.
const CROWDED_PERCENT: i64 = 90;

/// One replica's view of the primary's load.
#[derive(Debug, Clone)]
pub struct PgLoad {
    pool: DieselPgPool,
    crowded: Arc<AtomicBool>,
}

#[derive(diesel::QueryableByName)]
struct Connections {
    #[diesel(sql_type = BigInt)]
    used: i64,
    #[diesel(sql_type = BigInt)]
    max: i64,
}

impl PgLoad {
    /// The load of the database `pool` reaches.
    pub fn new(pool: DieselPgPool) -> Self {
        Self { pool, crowded: Arc::new(AtomicBool::new(false)) }
    }

    /// Count the database's client connections once.
    pub async fn measure(&self) -> Result<(), StoreError> {
        let mut conn = self.pool.get().await.map_err(pool_be)?;
        let c: Connections = diesel::sql_query(
            "SELECT count(*) AS used, current_setting('max_connections')::bigint AS max
             FROM pg_stat_activity WHERE backend_type = 'client backend'",
        )
        .get_result(&mut conn)
        .await
        .map_err(diesel_be)?;
        self.crowded.store(crowded(&c), Ordering::Relaxed);
        Ok(())
    }

    /// Count the connections every [`WINDOW`] until the task is aborted.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut every = tokio::time::interval(WINDOW);
            loop {
                every.tick().await;
                if let Err(e) = self.measure().await {
                    tracing::warn!(error = %e, "store: could not count the database's connections");
                }
            }
        })
    }
}

fn crowded(c: &Connections) -> bool {
    c.used * 100 > c.max * CROWDED_PERCENT
}

impl DatabaseLoad for PgLoad {
    fn overloaded(&self) -> bool {
        self.pool.slow_within(WINDOW) || AtomicBool::load(&self.crowded, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_database_is_crowded_past_90_percent_of_max_connections() {
        assert!(!crowded(&Connections { used: 90, max: 100 }));
        assert!(crowded(&Connections { used: 91, max: 100 }));
        assert!(!crowded(&Connections { used: 9, max: 10 }));
        assert!(crowded(&Connections { used: 10, max: 10 }));
    }
}
