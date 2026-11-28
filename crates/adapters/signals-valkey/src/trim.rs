//! Trimming. An entry stays in its stream after it is acknowledged, so every
//! [`EVERY`] each replica trims each stream it reads below the oldest entry a
//! group still needs: the group's oldest pending entry, else the last one it
//! was delivered. Debezium's heartbeat stream, which no group reads, keeps its
//! last [`HEARTBEATS`] entries; a feed stream no agentd handler reads keeps what
//! its readers' groups need, or its last [`UNREAD`] while no group reads it.

use std::time::Duration;

use redis::streams::{StreamInfoGroupsReply, StreamPendingReply, StreamTrimOptions, StreamTrimmingMode};
use redis::{AsyncCommands, RedisResult};

use crate::valkey::{Conn, Valkey};

/// How often a stream is trimmed.
const EVERY: Duration = Duration::from_secs(60);
/// The heartbeat entries kept.
const HEARTBEATS: usize = 100;
/// How far back a stream no group reads is kept.
pub(crate) const UNREAD: Duration = Duration::from_secs(7 * 24 * 3600);

/// What a trim keeps.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Keep {
    /// Every entry a group still needs; nothing is trimmed while no group reads.
    Needed,
    /// The last entries, this many.
    Last(usize),
    /// Every entry a group still needs; while no group reads, the entries newer than this.
    NeededOrNewer(Duration),
}

impl Keep {
    /// The heartbeat stream's.
    pub(crate) const HEARTBEATS: Self = Self::Last(HEARTBEATS);
}

/// Trim `stream` every [`EVERY`], for as long as the task runs.
pub(crate) async fn every(valkey: Valkey, stream: String, keep: Keep) {
    loop {
        tokio::time::sleep(EVERY).await;
        if let Err(e) = trim(&valkey, &stream, keep).await {
            tracing::warn!(stream = %stream, error = %e, "changes: a stream trim failed; it runs again");
        }
    }
}

/// Trim `stream` once; the entries removed.
pub(crate) async fn trim(valkey: &Valkey, stream: &str, keep: Keep) -> RedisResult<usize> {
    let mut conn = valkey.connection().await?;
    let exists: bool = conn.exists(stream).await?;
    let options = match (exists, keep) {
        (false, _) => return Ok(0),
        (true, Keep::Last(n)) => StreamTrimOptions::maxlen(StreamTrimmingMode::Exact, n),
        (true, Keep::Needed) => match needed(&mut conn, stream).await? {
            Some((ms, seq)) => StreamTrimOptions::minid(StreamTrimmingMode::Exact, format!("{ms}-{seq}")),
            None => return Ok(0),
        },
        (true, Keep::NeededOrNewer(age)) => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            let newer_than = u64::try_from(now.saturating_sub(age).as_millis()).unwrap_or(0);
            let (ms, seq) = needed(&mut conn, stream).await?.unwrap_or((newer_than, 0));
            StreamTrimOptions::minid(StreamTrimmingMode::Exact, format!("{ms}-{seq}"))
        }
    };
    conn.xtrim_options(stream, &options).await
}

/// The oldest entry some group of `stream` still needs; `None` when no group reads it.
async fn needed(conn: &mut Conn, stream: &str) -> RedisResult<Option<(u64, u64)>> {
    let info: StreamInfoGroupsReply = conn.xinfo_groups(stream).await?;
    let mut oldest = None;
    for group in info.groups {
        let pending: StreamPendingReply = match group.pending {
            0 => StreamPendingReply::Empty,
            _ => conn.xpending(stream, &group.name).await?,
        };
        let id = match pending {
            StreamPendingReply::Data(data) => data.start_id,
            _ => group.last_delivered_id,
        };
        let id = parse(&id);
        oldest = Some(oldest.map_or(id, |o: (u64, u64)| o.min(id)));
    }
    Ok(oldest)
}

/// An entry id `<ms>-<seq>`; one that does not parse keeps everything.
fn parse(id: &str) -> (u64, u64) {
    let parsed = id.split_once('-').and_then(|(ms, seq)| Some((ms.parse().ok()?, seq.parse().ok()?)));
    parsed.unwrap_or((0, 0))
}

#[cfg(test)]
mod tests;
