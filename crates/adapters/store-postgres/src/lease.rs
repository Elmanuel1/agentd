//! Session leases on Postgres: a session-level advisory lock held on its own
//! connection for as long as the lease. The lock goes with the connection,
//! so a replica that dies loses its leases at once.

use async_trait::async_trait;
use diesel::sql_types::{Bool, Text};
use diesel_async::RunQueryDsl;
use shared::ports::lease::{LeaseGuard, Leases};
use shared::BackendError;

use crate::{PgBackend, PgError, PoolSettings, PooledConnection};

#[derive(diesel::QueryableByName)]
struct Locked {
    #[diesel(sql_type = Bool)]
    ok: bool,
}

/// Leases on a pool of their own, so held leases never starve queries.
#[derive(Debug, Clone)]
pub struct PgLeases {
    backend: PgBackend,
}

impl PgLeases {
    /// At most `max_held` leases at once on `database_url`.
    pub async fn connect(database_url: &str, max_held: usize) -> Result<Self, PgError> {
        let settings = PoolSettings { max_size: max_held, ..PoolSettings::default() };
        Ok(Self { backend: PgBackend::connect_with(database_url, settings).await? })
    }
}

/// Unlocks when dropped, then gives the connection back; a connection whose
/// unlock fails is closed instead, which releases the lock.
struct Held {
    conn: Option<PooledConnection>,
    key: String,
}

impl Drop for Held {
    fn drop(&mut self) {
        let Some(mut conn) = self.conn.take() else { return };
        let key = std::mem::take(&mut self.key);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let unlocked = diesel::sql_query("SELECT pg_advisory_unlock(hashtextextended($1, 0)) AS ok")
                    .bind::<Text, _>(&key)
                    .get_result::<Locked>(&mut conn)
                    .await;
                if !unlocked.is_ok_and(|l| l.ok) {
                    let _ = PooledConnection::take(conn);
                }
            });
        } else {
            let _ = PooledConnection::take(conn);
        }
    }
}

#[async_trait]
impl Leases for PgLeases {
    async fn try_acquire(&self, key: &str) -> Result<Option<LeaseGuard>, BackendError> {
        let mut conn = self.backend.diesel.get().await.map_err(BackendError::new)?;
        let locked: Locked = diesel::sql_query("SELECT pg_try_advisory_lock(hashtextextended($1, 0)) AS ok")
            .bind::<Text, _>(key)
            .get_result(&mut conn)
            .await
            .map_err(BackendError::new)?;
        Ok(locked.ok.then(|| LeaseGuard::new(Held { conn: Some(conn), key: key.to_owned() })))
    }
}
