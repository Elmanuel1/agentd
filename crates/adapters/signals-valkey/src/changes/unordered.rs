//! An unordered stream: what stopped replicas left, then new entries, read
//! 16 at a time and handled together. Each entry is acknowledged as it
//! finishes, so a fast one is never left pending behind a slow one past the
//! 60 s after which another replica claims it.

use futures::StreamExt;
use redis::streams::StreamId;
use redis::{AsyncCommands, RedisResult};

use super::{Consumer, GROUP, RECLAIM_MS, ValkeyChanges};
use crate::valkey::Conn;

/// Entries one unordered read takes.
pub(super) const UNORDERED_BATCH: usize = 16;

impl ValkeyChanges {
    /// Claim what stopped replicas left and handle it, then read new entries
    /// and handle those: a failed read never drops what was claimed.
    pub(super) async fn pass(
        &self,
        conn: &mut Conn,
        stream: &str,
        handler: &dyn Consumer,
    ) -> RedisResult<()> {
        let stale = self.claim(conn, stream, RECLAIM_MS, UNORDERED_BATCH).await?;
        handle(conn, stream, handler, stale).await?;
        let fresh = self.read(conn, stream, UNORDERED_BATCH).await?;
        handle(conn, stream, handler, fresh).await
    }
}

/// Handle `entries` together, acknowledging each that needs no more handling as it finishes.
async fn handle(
    conn: &mut Conn,
    stream: &str,
    handler: &dyn Consumer,
    entries: Vec<StreamId>,
) -> RedisResult<()> {
    let mut handling: futures::stream::FuturesUnordered<_> =
        entries.iter().map(|e| async move { (e.id.as_str(), handler.consume(e).await) }).collect();
    while let Some((id, done)) = handling.next().await {
        if done {
            let _: usize = conn.xack(stream, GROUP, &[id]).await?;
        }
    }
    Ok(())
}
