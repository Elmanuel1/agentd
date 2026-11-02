//! Connecting and pooling: the pool's size and wait, how a connection is
//! checked out, and the observer that watches every statement.

use std::sync::Arc;

use diesel::connection::Instrumentation;
use diesel_async::pooled_connection::deadpool::Pool as DieselPool;
use diesel_async::pooled_connection::{AsyncDieselConnectionManager, ManagerConfig, RecyclingMethod};
use diesel_async::{AsyncConnection, AsyncPgConnection};

use crate::{PgBackend, PgError};

/// Watches the pool: every statement a connection runs, and every
/// connection that cannot be opened (no statement runs then).
pub trait PgObserver: Send + Sync {
    /// The observer of one new connection's statements.
    fn statements(&self) -> Box<dyn Instrumentation>;
    /// A pooled connection could not be opened.
    fn connect_failed(&self, error: &diesel::ConnectionError);
}

/// The pool's size, and how long a caller waits for a connection before the
/// request fails instead of queueing without end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSettings {
    /// Connections per replica.
    pub max_size: usize,
    /// The longest wait for a free connection.
    pub wait: std::time::Duration,
}

impl Default for PoolSettings {
    fn default() -> Self {
        Self { max_size: 10, wait: std::time::Duration::from_secs(5) }
    }
}

impl PgBackend {
    /// Connect using `database_url` (or `DATABASE_URL`), pooled with the
    /// default [`PoolSettings`].
    pub async fn connect(database_url: &str) -> Result<Self, PgError> {
        Self::connect_with(database_url, PoolSettings::default()).await
    }

    /// Connect using `database_url`, pooled as `settings` says.
    pub async fn connect_with(database_url: &str, settings: PoolSettings) -> Result<Self, PgError> {
        let manager = AsyncDieselConnectionManager::new_with_config(database_url, manager_config());
        Self::pool(manager, settings)
    }

    /// [`Self::connect_with`], with `observe` watching the pool.
    pub async fn connect_observed(
        database_url: &str,
        settings: PoolSettings,
        observe: Arc<dyn PgObserver>,
    ) -> Result<Self, PgError> {
        let mut config = manager_config();
        config.custom_setup = Box::new(move |url| {
            let observe = observe.clone();
            let url = url.to_owned();
            Box::pin(async move {
                let mut conn =
                    AsyncPgConnection::establish(&url).await.inspect_err(|e| observe.connect_failed(e))?;
                conn.set_instrumentation(observe.statements());
                Ok(conn)
            })
        });
        Self::pool(AsyncDieselConnectionManager::new_with_config(database_url, config), settings)
    }

    fn pool(
        manager: AsyncDieselConnectionManager<AsyncPgConnection>,
        settings: PoolSettings,
    ) -> Result<Self, PgError> {
        let diesel = DieselPool::builder(manager)
            .max_size(settings.max_size)
            .wait_timeout(Some(settings.wait))
            .runtime(deadpool::Runtime::Tokio1)
            .build()
            .map_err(|e| PgError::Connect(e.to_string()))?;
        Ok(Self { diesel })
    }
}

/// Checkout without a test query: the default (`Verified`) runs `SELECT 1`
/// on every checkout, a round trip per store call. A connection that broke
/// while idle fails on its first statement instead.
fn manager_config() -> ManagerConfig<AsyncPgConnection> {
    let mut config = ManagerConfig::default();
    config.recycling_method = RecyclingMethod::Fast;
    config
}
