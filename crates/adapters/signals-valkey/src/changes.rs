//! The change feed over Valkey Streams: Debezium Server writes each table's
//! committed changes to the stream `<prefix>.public.<table>` (its Redis
//! sink, `extended` format), an external scheduler writes its fires to
//! `<prefix>.fires`, and the replicas read them in the consumer group
//! `agentd`. An entry is acknowledged once its handler succeeds; an
//! entry left pending by a replica that stopped is claimed by another after
//! [`RECLAIM_MS`]. An ordered handler's stream is read by one replica at a
//! time, which holds a lease in Valkey, in entry order, stopping at a failure.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use redis::aio::MultiplexedConnection;
use redis::streams::{
    StreamAutoClaimOptions, StreamAutoClaimReply, StreamId, StreamReadOptions, StreamReadReply,
};
use redis::{AsyncCommands, Client, RedisError, RedisResult, SetExpiry, SetOptions};
use shared::ports::changes::{Change, ChangeHandler};
use shared::ports::fire::FireRegistry;

use crate::fires::Fires;

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
    client: Client,
    prefix: String,
    consumer: String,
}

impl std::fmt::Debug for ValkeyChanges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValkeyChanges").field("prefix", &self.prefix).finish_non_exhaustive()
    }
}

impl ValkeyChanges {
    /// The streams of Debezium's `prefix` (its `topic.prefix`) on the Valkey
    /// at `url`, read as `consumer`, this replica's name.
    pub fn new(url: &str, prefix: &str, consumer: &str) -> Result<Self, RedisError> {
        Ok(Self { client: Client::open(url)?, prefix: prefix.to_owned(), consumer: consumer.to_owned() })
    }

    /// Read each handler's stream on a task of its own.
    pub fn spawn(&self, handlers: Vec<Arc<dyn ChangeHandler>>) -> Vec<tokio::task::JoinHandle<()>> {
        handlers
            .into_iter()
            .map(|h| {
                let stream = format!("{}.public.{}", self.prefix, h.table());
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
        loop {
            if let Err(e) = self.serve(&stream, consumer.as_ref()).await {
                tracing::warn!(stream = %stream, error = %e, "changes: the stream read failed; reconnecting");
            }
            tokio::time::sleep(BACKOFF).await;
        }
    }

    async fn serve(&self, stream: &str, handler: &dyn Consumer) -> RedisResult<()> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
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

    /// Claim what stopped replicas left, then read new entries; handle each.
    async fn pass(
        &self,
        conn: &mut MultiplexedConnection,
        stream: &str,
        handler: &dyn Consumer,
    ) -> RedisResult<()> {
        let stale = self.claim(conn, stream, RECLAIM_MS).await?;
        let fresh = self.read(conn, stream).await?;
        for entry in stale.into_iter().chain(fresh) {
            if handler.consume(&entry).await {
                let _: usize = conn.xack(stream, GROUP, &[&entry.id]).await?;
            }
        }
        Ok(())
    }

    /// Under the lease only: every pending entry, then new ones, in order,
    /// stopping at the first failure so nothing overtakes it.
    async fn ordered_pass(
        &self,
        conn: &mut MultiplexedConnection,
        stream: &str,
        handler: &dyn Consumer,
    ) -> RedisResult<()> {
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
    async fn lease(&self, conn: &mut MultiplexedConnection, stream: &str) -> RedisResult<bool> {
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

    async fn claim(
        &self,
        conn: &mut MultiplexedConnection,
        stream: &str,
        idle_ms: usize,
    ) -> RedisResult<Vec<StreamId>> {
        let options = StreamAutoClaimOptions::default().count(BATCH);
        let reply: StreamAutoClaimReply =
            conn.xautoclaim_options(stream, GROUP, &self.consumer, idle_ms, "0", options).await?;
        Ok(reply.claimed)
    }

    async fn read(&self, conn: &mut MultiplexedConnection, stream: &str) -> RedisResult<Vec<StreamId>> {
        let options = StreamReadOptions::default().group(GROUP, &self.consumer).count(BATCH).block(BLOCK_MS);
        let reply: StreamReadReply = conn.xread_options(&[stream], &[">"], &options).await?;
        Ok(reply.keys.into_iter().flat_map(|k| k.ids).collect())
    }
}

/// A table's changes, as Debezium writes them.
struct Changes(Arc<dyn ChangeHandler>);

#[async_trait]
impl Consumer for Changes {
    fn ordered(&self) -> bool {
        self.0.ordered()
    }

    /// Done once handled, or when the entry is not a change at all (a tombstone).
    async fn consume(&self, entry: &StreamId) -> bool {
        let value: Option<String> = entry.get("value");
        let change =
            value.and_then(|v| serde_json::from_str(&v).ok()).and_then(|v| Change::from_debezium(&v));
        let Some(change) = change else { return true };
        match self.0.handle(&change).await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(table = %change.table, id = %entry.id, error = %e, "changes: a handler failed; it runs again");
                false
            }
        }
    }
}
