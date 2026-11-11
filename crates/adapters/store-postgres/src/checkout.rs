//! Taking a connection from the pool, under one deadline.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use diesel::sql_types::Text;
use diesel_async::pooled_connection::deadpool::Pool as DieselPool;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::Writer;

/// The connection pool (diesel-async over tokio-postgres), with one deadline
/// on a whole checkout: deadpool bounds the wait, the create and the recycle
/// separately and retries after a recycle times out, so on their own those
/// bounds can add up past the configured wait.
#[derive(Clone)]
pub struct DieselPgPool {
    inner: DieselPool<AsyncPgConnection>,
    deadline: Duration,
    slow: Arc<SlowCheckouts>,
}

/// A checkout that waits longer than this is slow.
const SLOW_CHECKOUT: Duration = Duration::from_secs(1);

/// When a checkout last waited longer than [`SLOW_CHECKOUT`].
struct SlowCheckouts {
    since: Instant,
    /// Milliseconds after `since`, plus one; 0 while none has.
    last: AtomicU64,
}

impl std::fmt::Debug for DieselPgPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DieselPgPool").field("deadline", &self.deadline).finish_non_exhaustive()
    }
}

impl DieselPgPool {
    pub(crate) fn new(inner: DieselPool<AsyncPgConnection>, deadline: Duration) -> Self {
        let slow = Arc::new(SlowCheckouts { since: Instant::now(), last: AtomicU64::new(0) });
        Self { inner, deadline, slow }
    }

    /// A connection, or a wait timeout once the deadline passes.
    pub async fn get(&self) -> Result<PooledConnection, PoolError> {
        let started = Instant::now();
        let conn = tokio::time::timeout(self.deadline, self.inner.get())
            .await
            .unwrap_or(Err(PoolError::Timeout(deadpool::managed::TimeoutType::Wait)));
        if started.elapsed() > SLOW_CHECKOUT {
            let at = u64::try_from(self.slow.since.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.slow.last.store(at.saturating_add(1), Ordering::Relaxed);
        }
        conn
    }

    /// A checkout waited longer than [`SLOW_CHECKOUT`] within the last `window`.
    pub fn slow_within(&self, window: Duration) -> bool {
        let last = AtomicU64::load(&self.slow.last, Ordering::Relaxed);
        let now = u64::try_from(self.slow.since.elapsed().as_millis()).unwrap_or(u64::MAX);
        last > 0 && now.saturating_sub(last - 1) <= u64::try_from(window.as_millis()).unwrap_or(u64::MAX)
    }

    /// A connection for a write, carrying the current [`Writer`] so every row
    /// the write adds records who wrote it.
    pub async fn write(&self) -> Result<PooledConnection, PoolError> {
        let mut conn = self.get().await?;
        let writer = Writer::current();
        // Session-level (`false`): most writes run outside an explicit
        // transaction, where a transaction-local setting would be gone before
        // the write's own statement ran. Every write checkout sets both again.
        diesel::sql_query(
            "SELECT set_config('agentd.actor', $1, false), set_config('agentd.request_id', $2, false)",
        )
        .bind::<Text, _>(&writer.actor)
        .bind::<Text, _>(&writer.request_id)
        .execute(&mut *conn)
        .await
        .map_err(|e| PoolError::Backend(diesel_async::pooled_connection::PoolError::QueryError(e)))?;
        Ok(conn)
    }

    /// Connections open, idle and waited for.
    pub fn status(&self) -> deadpool::managed::Status {
        self.inner.status()
    }
}

/// A pooled connection.
pub type PooledConnection = diesel_async::pooled_connection::deadpool::Object<AsyncPgConnection>;
type PoolError = diesel_async::pooled_connection::deadpool::PoolError;
