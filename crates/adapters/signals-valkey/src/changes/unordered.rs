//! An unordered stream: what stopped replicas left, then new entries, read
//! 16 at a time and handled together, so a batch takes as long as its
//! slowest entry and ends well inside the 60 s after which another replica
//! claims what is pending.

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

/// Handle `entries` together, acknowledging those that need no more handling.
async fn handle(
    conn: &mut Conn,
    stream: &str,
    handler: &dyn Consumer,
    entries: Vec<StreamId>,
) -> RedisResult<()> {
    let handled = futures::future::join_all(entries.iter().map(|e| handler.consume(e))).await;
    let done: Vec<&str> =
        entries.iter().zip(handled).filter(|(_, ok)| *ok).map(|(e, _)| e.id.as_str()).collect();
    if done.is_empty() {
        return Ok(());
    }
    let _: usize = conn.xack(stream, GROUP, &done).await?;
    Ok(())
}
