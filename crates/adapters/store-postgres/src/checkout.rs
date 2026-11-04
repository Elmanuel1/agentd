//! Taking a connection from the pool, under one deadline.

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
    deadline: std::time::Duration,
}

impl std::fmt::Debug for DieselPgPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DieselPgPool").field("deadline", &self.deadline).finish_non_exhaustive()
    }
}

impl DieselPgPool {
    pub(crate) fn new(inner: DieselPool<AsyncPgConnection>, deadline: std::time::Duration) -> Self {
        Self { inner, deadline }
    }

    /// A connection, or a wait timeout once the deadline passes.
    pub async fn get(&self) -> Result<PooledConnection, PoolError> {
        tokio::time::timeout(self.deadline, self.inner.get())
            .await
            .unwrap_or(Err(PoolError::Timeout(deadpool::managed::TimeoutType::Wait)))
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
