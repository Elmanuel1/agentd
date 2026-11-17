//! The change feed over Valkey Streams: Debezium Server writes each table's committed changes to the
//! stream `<prefix>.public.<table>` and the logical messages to `<prefix>.message` (its Redis sink,
//! `extended` format), an external scheduler writes its fires to `<prefix>.fires`, and the replicas read
//! them in the consumer group `agentd`. An entry is acknowledged once its handler succeeds; an entry left
//! pending by a replica that stopped is claimed by another after [`RECLAIM_MS`], and each stream is
//! trimmed of what every group has acknowledged ([`crate::trim`]). An ordered handler's stream is read
//! by one replica at a time, which holds a lease in Valkey, in entry order, stopping at a failure.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use redis::streams::{
    StreamAutoClaimOptions, StreamAutoClaimReply, StreamId, StreamReadOptions, StreamReadReply,
};
use redis::{AsyncCommands, RedisResult, SetExpiry, SetOptions};
use shared::ports::changes::{self, ChangeHandler};
use shared::ports::fire::FireRegistry;

use crate::fires::Fires;
use crate::trim::{self, Keep};
use crate::valkey::{Conn, Valkey};

/// The consumer group every replica reads in.
const GROUP: &str = "agentd";
/// An entry pending this long is claimed by another replica.
const RECLAIM_MS: usize = 60_000;
/// How long a read waits for new entries.
const BLOCK_MS: usize = 5_000;
/// Entries one read takes.
const BATCH: usize = 100;
/// An ordered stream's lease, renewed on every pass.
const LEASE_MS: u64 = 30_000;
/// The pause after a connection or handler failure.
const BACKOFF: Duration = Duration::from_secs(1);

/// What reads one stream.
#[async_trait]
pub(crate) trait Consumer: Send + Sync {
    /// Whether its entries are read one at a time, in order.
    fn ordered(&self) -> bool;
    /// Handle one entry; `true` once it needs no more handling.
    async fn consume(&self, entry: &StreamId) -> bool;
}

/// Reads the change streams of one Valkey.
#[derive(Clone)]
pub struct ValkeyChanges {
    valkey: Valkey,
    prefix: String,
    consumer: String,
}

impl std::fmt::Debug for ValkeyChanges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValkeyChanges").field("prefix", &self.prefix).finish_non_exhaustive()
    }
}

impl ValkeyChanges {
    /// The streams of Debezium's `prefix` (its `topic.prefix`) on `valkey`,
    /// read as `consumer`, this replica's name.
    pub fn new(valkey: &Valkey, prefix: &str, consumer: &str) -> Self {
        Self { valkey: valkey.clone(), prefix: prefix.to_owned(), consumer: consumer.to_owned() }
    }

    /// Read each handler's stream on a task of its own, and trim Debezium's
    /// heartbeats.
    pub fn spawn(&self, handlers: Vec<Arc<dyn ChangeHandler>>) -> Vec<tokio::task::JoinHandle<()>> {
        let heartbeats = format!("__debezium-heartbeat.{}", self.prefix);
        drop(tokio::spawn(trim::every(self.valkey.clone(), heartbeats, Keep::HEARTBEATS)));
        handlers
            .into_iter()
            .map(|h| {
                let stream = h.source().topic(&self.prefix);
                tokio::spawn(self.clone().run(stream, Arc::new(Changes(h))))
            })
            .collect()
    }

    /// Run the fires an external scheduler writes to `<prefix>.fires`.
    pub fn spawn_fires(&self, fires: Arc<FireRegistry>) -> tokio::task::JoinHandle<()> {
        let stream = format!("{}.fires", self.prefix);
        tokio::spawn(self.clone().run(stream, Arc::new(Fires::new(fires))))
    }

    async fn run(self, stream: String, consumer: Arc<dyn Consumer>) {
        drop(tokio::spawn(trim::every(self.valkey.clone(), stream.clone(), Keep::Needed)));
        loop {
            if let Err(e) = self.serve(&stream, consumer.as_ref()).await {
                tracing::warn!(stream = %stream, error = %e, "changes: the stream read failed; reconnecting");
            }
            tokio::time::sleep(BACKOFF).await;
        }
    }

    async fn serve(&self, stream: &str, handler: &dyn Consumer) -> RedisResult<()> {
        let mut conn = self.valkey.connection().await?;
        let created: RedisResult<()> = conn.xgroup_create_mkstream(stream, GROUP, "0").await;
        match created {
            Err(e) if e.code() != Some("BUSYGROUP") => return Err(e),
            _ => {}
        }
        loop {
            match handler.ordered() {
                true => self.ordered_pass(&mut conn, stream, handler).await?,
                false => self.pass(&mut conn, stream, handler).await?,
            }
        }
    }

    /// Claim what stopped replicas left and handle it, then read new entries
    /// and handle those: a failed read never drops what was claimed.
    async fn pass(&self, conn: &mut Conn, stream: &str, handler: &dyn Consumer) -> RedisResult<()> {
        let stale = self.claim(conn, stream, RECLAIM_MS).await?;
        handle(conn, stream, handler, stale).await?;
        let fresh = self.read(conn, stream).await?;
        handle(conn, stream, handler, fresh).await
    }

    /// Under the lease only: every pending entry, then new ones, in order,
    /// stopping at the first failure so nothing overtakes it.
    async fn ordered_pass(&self, conn: &mut Conn, stream: &str, handler: &dyn Consumer) -> RedisResult<()> {
        if !self.lease(conn, stream).await? {
            tokio::time::sleep(Duration::from_millis(LEASE_MS / 3)).await;
            return Ok(());
        }
        let mut entries = self.claim(conn, stream, 0).await?;
        if entries.is_empty() {
            entries = self.read(conn, stream).await?;
        }
        for entry in entries {
            if !handler.consume(&entry).await {
                tokio::time::sleep(BACKOFF).await;
                return Ok(());
            }
            let _: usize = conn.xack(stream, GROUP, &[&entry.id]).await?;
        }
        Ok(())
    }

    /// Take or renew the stream's lease; `false` when another replica holds it.
    async fn lease(&self, conn: &mut Conn, stream: &str) -> RedisResult<bool> {
        let key = format!("agentd:lease:{stream}");
        let held: Option<String> = conn.get(&key).await?;
        if held.as_deref() == Some(self.consumer.as_str()) {
            let _: bool = conn.pexpire(&key, i64::try_from(LEASE_MS).unwrap_or(i64::MAX)).await?;
            return Ok(true);
        }
        let options = SetOptions::default()
            .conditional_set(redis::ExistenceCheck::NX)
            .with_expiration(SetExpiry::PX(LEASE_MS));
        let taken: Option<String> = conn.set_options(&key, &self.consumer, options).await?;
        Ok(taken.is_some())
    }

    async fn claim(&self, conn: &mut Conn, stream: &str, idle_ms: usize) -> RedisResult<Vec<StreamId>> {
        let options = StreamAutoClaimOptions::default().count(BATCH);
        let reply: StreamAutoClaimReply =
            conn.xautoclaim_options(stream, GROUP, &self.consumer, idle_ms, "0", options).await?;
        Ok(reply.claimed)
    }

    async fn read(&self, conn: &mut Conn, stream: &str) -> RedisResult<Vec<StreamId>> {
        let options = StreamReadOptions::default().group(GROUP, &self.consumer).count(BATCH).block(BLOCK_MS);
        let reply: StreamReadReply = conn.xread_options(&[stream], &[">"], &options).await?;
        Ok(reply.keys.into_iter().flat_map(|k| k.ids).collect())
    }
}

/// Handle each of `entries`, acknowledging those that need no more handling.
async fn handle(
    conn: &mut Conn,
    stream: &str,
    handler: &dyn Consumer,
    entries: Vec<StreamId>,
) -> RedisResult<()> {
    for entry in entries {
        if handler.consume(&entry).await {
            let _: usize = conn.xack(stream, GROUP, &[&entry.id]).await?;
        }
    }
    Ok(())
}

/// A table's changes, as Debezium writes them.
struct Changes(Arc<dyn ChangeHandler>);

#[async_trait]
impl Consumer for Changes {
    fn ordered(&self) -> bool {
        self.0.ordered()
    }

    async fn consume(&self, entry: &StreamId) -> bool {
        let value: Option<String> = entry.get("value");
        changes::deliver(self.0.as_ref(), value.as_deref(), &entry.id).await
    }
}
