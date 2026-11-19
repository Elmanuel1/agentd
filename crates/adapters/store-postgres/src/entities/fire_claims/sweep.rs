//! Retention of the fire queue: a fire's history is its deployment run, so a
//! done row is deleted 30 days after its occurrence, as is a row the earlier
//! fire inbox left unfinished, which no worker runs.

use std::time::Duration;

use diesel_async::RunQueryDsl;
use shared::ports::fire::FireError;

use super::{PgFireClaims, unavailable};

/// How often the queue is swept.
const EVERY: Duration = Duration::from_secs(3600);

impl PgFireClaims {
    /// Delete the rows older than 30 days that are done or that no worker runs; how many.
    pub async fn sweep(&self) -> Result<usize, FireError> {
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        let done = diesel::sql_query(
            "DELETE FROM fire_claims WHERE done AND scheduled_at < now() - interval '30 days'",
        )
        .execute(&mut conn)
        .await
        .map_err(unavailable)?;
        let stranded = diesel::sql_query(
            "DELETE FROM fire_claims WHERE NOT done AND due_at IS NULL AND scheduled_at < now() - interval '30 days'",
        )
        .execute(&mut conn)
        .await
        .map_err(unavailable)?;
        Ok(done + stranded)
    }

    /// Sweep every hour, until the task is aborted.
    pub fn spawn_sweep(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut every = tokio::time::interval(EVERY);
            loop {
                every.tick().await;
                if let Err(e) = self.sweep().await {
                    tracing::warn!(error = %e, "fires: the fire queue's sweep failed");
                }
            }
        })
    }
}
