//! One Valkey, standalone or a cluster (`AGENTD_VALKEY_CLUSTER`): the
//! connections the signals, the budget and the change streams run their
//! commands on, and the subscription the signals arrive on. A cluster is
//! reached through its configuration endpoint and speaks RESP3, which its
//! pub/sub needs; it follows the slots, reconnects and resubscribes itself.

use std::sync::Arc;

use futures::stream::BoxStream;
use futures::StreamExt;
use redis::aio::{ConnectionLike, ConnectionManager, ConnectionManagerConfig, MultiplexedConnection};
use redis::cluster::{ClusterClient, ClusterClientBuilder};
use redis::cluster_async::ClusterConnection;
use redis::{Client, Cmd, Pipeline, ProtocolVersion, PushKind, RedisError, RedisFuture, RedisResult, Value};
use tokio::sync::OnceCell;

/// A standalone Valkey or a cluster.
#[derive(Clone)]
pub struct Valkey {
    url: String,
    node: Node,
}

#[derive(Clone)]
enum Node {
    Single(Box<Client>),
    Cluster(Arc<Cluster>),
}

/// A cluster's client, and the connection its short commands share.
pub(crate) struct Cluster {
    client: ClusterClient,
    shared: OnceCell<ClusterConnection>,
}

impl std::fmt::Debug for Valkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cluster = matches!(self.node, Node::Cluster(..));
        f.debug_struct("Valkey").field("cluster", &cluster).finish_non_exhaustive()
    }
}

impl Valkey {
    /// The Valkey at `url`, a cluster's configuration endpoint when `cluster`.
    /// Nothing connects until first used, so a replica starts while Valkey is down.
    pub fn open(url: &str, cluster: bool) -> Result<Self, RedisError> {
        let node = match cluster {
            true => {
                Node::Cluster(Arc::new(Cluster { client: cluster_client(url)?, shared: OnceCell::new() }))
            }
            false => Node::Single(Box::new(Client::open(url)?)),
        };
        Ok(Self { url: url.to_owned(), node })
    }

    /// A connection of its own, for blocking reads.
    pub(crate) async fn connection(&self) -> RedisResult<Conn> {
        match &self.node {
            Node::Single(client) => client.get_multiplexed_async_connection().await.map(Conn::Single),
            Node::Cluster(cluster) => cluster.client.get_async_connection().await.map(Conn::Cluster),
        }
    }

    /// A connection shared by short commands, which reconnects by itself.
    pub(crate) fn shared(&self) -> Result<Shared, RedisError> {
        Ok(match &self.node {
            Node::Single(client) => Shared::Single(ConnectionManager::new_lazy_with_config(
                client.as_ref().clone(),
                ConnectionManagerConfig::new(),
            )?),
            Node::Cluster(cluster) => Shared::Cluster(cluster.clone()),
        })
    }

    /// The payloads published on `channel`, until the subscription drops.
    pub(crate) async fn subscribe(&self, channel: &str) -> RedisResult<BoxStream<'static, String>> {
        match &self.node {
            Node::Single(client) => {
                let mut pubsub = client.get_async_pubsub().await?;
                pubsub.subscribe(channel).await?;
                let messages = pubsub.into_on_message().filter_map(|m| async move { m.get_payload().ok() });
                Ok(messages.boxed())
            }
            Node::Cluster(..) => {
                let (pushes, received) = tokio::sync::mpsc::unbounded_channel();
                let client = ClusterClientBuilder::new([self.url.as_str()])
                    .use_protocol(ProtocolVersion::RESP3)
                    .push_sender(pushes)
                    .build()?;
                let mut conn = client.get_async_connection().await?;
                conn.subscribe(channel).await?;
                // The stream holds the connection, and with it the subscription.
                let pushes = futures::stream::unfold((received, conn), |(mut received, conn)| async move {
                    received.recv().await.map(|push| (push, (received, conn)))
                });
                let messages = pushes.filter_map(|push| {
                    let payload = match (push.kind, push.data.get(1)) {
                        (PushKind::Message, Some(data)) => redis::from_redis_value_ref(data).ok(),
                        _ => None,
                    };
                    async move { payload }
                });
                Ok(messages.boxed())
            }
        }
    }
}

fn cluster_client(url: &str) -> Result<ClusterClient, RedisError> {
    ClusterClientBuilder::new([url]).use_protocol(ProtocolVersion::RESP3).build()
}

/// A connection the commands run on.
pub(crate) enum Conn {
    Single(MultiplexedConnection),
    Cluster(ClusterConnection),
}

/// The connection short commands share.
#[derive(Clone)]
pub(crate) enum Shared {
    Single(ConnectionManager),
    Cluster(Arc<Cluster>),
}

impl Shared {
    /// The connection, made on first use; a failed first use tries again next time.
    pub(crate) async fn get(&self) -> RedisResult<SharedConn> {
        match self {
            Self::Single(manager) => Ok(SharedConn::Single(manager.clone())),
            Self::Cluster(cluster) => {
                let conn = cluster.shared.get_or_try_init(|| cluster.client.get_async_connection()).await?;
                Ok(SharedConn::Cluster(conn.clone()))
            }
        }
    }
}

/// A clone of the shared connection.
pub(crate) enum SharedConn {
    Single(ConnectionManager),
    Cluster(ClusterConnection),
}

macro_rules! delegate {
    ($ty:ident, $a:ident, $b:ident) => {
        impl ConnectionLike for $ty {
            fn req_packed_command<'a>(&'a mut self, cmd: &'a Cmd) -> RedisFuture<'a, Value> {
                match self {
                    Self::$a(c) => c.req_packed_command(cmd),
                    Self::$b(c) => c.req_packed_command(cmd),
                }
            }

            fn req_packed_commands<'a>(
                &'a mut self,
                pipeline: &'a Pipeline,
                offset: usize,
                count: usize,
            ) -> RedisFuture<'a, Vec<Value>> {
                match self {
                    Self::$a(c) => c.req_packed_commands(pipeline, offset, count),
                    Self::$b(c) => c.req_packed_commands(pipeline, offset, count),
                }
            }

            fn get_db(&self) -> i64 {
                match self {
                    Self::$a(c) => c.get_db(),
                    Self::$b(c) => c.get_db(),
                }
            }
        }
    };
}

delegate!(Conn, Single, Cluster);
delegate!(SharedConn, Single, Cluster);
