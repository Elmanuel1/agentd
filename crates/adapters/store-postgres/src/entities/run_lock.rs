//! A lock across replicas: a session-level advisory lock held on one pooled
//! connection for the length of a job.

use ::sessions::retention::RunLock;
use ::sessions::store::StoreError;
use async_trait::async_trait;
use diesel::sql_types::{Bool, Text};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use futures::future::BoxFuture;

use crate::{diesel_be, pool_be, PgBackend, PooledConnection};

/// [`RunLock`] on Postgres advisory locks.
#[derive(Debug, Clone)]
pub struct PgRunLock {
    backend: PgBackend,
}

impl PgRunLock {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

#[derive(diesel::QueryableByName)]
struct Won {
    #[diesel(sql_type = Bool)]
    won: bool,
}

/// The connection that holds the lock. Dropped without [`Held::release`]
/// (an error, a panic, a cancelled job), it leaves the pool and closes, and
/// Postgres frees the lock with it.
struct Held(Option<PooledConnection>);

impl Held {
    fn conn(&mut self) -> Result<&mut AsyncPgConnection, StoreError> {
        self.0.as_deref_mut().ok_or(StoreError::NotFound)
    }

    /// Back to the pool, holding nothing.
    fn release(mut self) {
        drop(self.0.take());
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(conn) = self.0.take() {
            drop(deadpool::managed::Object::take(conn));
        }
    }
}

#[async_trait]
impl RunLock for PgRunLock {
    async fn exclusive(
        &self,
        name: &str,
        job: BoxFuture<'_, Result<(), StoreError>>,
    ) -> Result<bool, StoreError> {
        let mut held = Held(Some(self.backend.diesel.get().await.map_err(pool_be)?));
        let won: Won = diesel::sql_query("SELECT pg_try_advisory_lock(hashtext($1)) AS won")
            .bind::<Text, _>(name)
            .get_result(held.conn()?)
            .await
            .map_err(diesel_be)?;
        if !won.won {
            held.release();
            return Ok(false);
        }
        job.await?;
        diesel::sql_query("SELECT pg_advisory_unlock(hashtext($1))")
            .bind::<Text, _>(name)
            .execute(held.conn()?)
            .await
            .map_err(diesel_be)?;
        held.release();
        Ok(true)
    }
}
