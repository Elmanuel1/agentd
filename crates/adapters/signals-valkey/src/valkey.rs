//! One Valkey, standalone or a cluster (`AGENTD_VALKEY_CLUSTER`): the
//! connections the signals, the budget and the change streams run their
//! commands on, and the subscription the signals arrive on. A cluster is
//! reached through its configuration endpoint and speaks RESP3, which its
//! pub/sub needs; it follows the slots, reconnects and resubscribes itself.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use redis::aio::{ConnectionLike, ConnectionManager, ConnectionManagerConfig, MultiplexedConnection};
use redis::cluster::{ClusterClientBuilder, ClusterConfig};
use redis::cluster_async::ClusterConnection;
use redis::{
    AsyncConnectionConfig, Client, Cmd, Pipeline, ProtocolVersion, PushKind, RedisError, RedisFuture,
    RedisResult, TlsCertificates, Value,
};

mod cluster;
use cluster::Cluster;

/// How long a blocking read's connection waits for an answer: longer than any
/// block (a change stream's 5 s), so only a hung Valkey times out, not a quiet stream.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// A standalone Valkey or a cluster.
#[derive(Clone)]
pub struct Valkey {
    url: String,
    root_cert: Option<Vec<u8>>,
    node: Node,
}

#[derive(Clone)]
enum Node {
    Single(Box<Client>),
    Cluster(Arc<Cluster>),
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
        Self::open_trusting(url, cluster, None)
    }

    /// [`Self::open`], a `rediss://` server's certificate chaining to the PEM
    /// file `ca` when one is given, else to the operating system's roots.
    pub fn open_trusting(url: &str, cluster: bool, ca: Option<&str>) -> Result<Self, RedisError> {
        let root_cert = ca.map(std::fs::read).transpose()?;
        let node = match (cluster, &root_cert) {
            (true, _) => Node::Cluster(Arc::new(Cluster::new(cluster_builder(url, &root_cert).build()?))),
            (false, Some(pem)) => {
                let certs = TlsCertificates { client_tls: None, root_cert: Some(pem.clone()) };
                Node::Single(Box::new(Client::build_with_tls(url, certs)?))
            }
            (false, None) => Node::Single(Box::new(Client::open(url)?)),
        };
        Ok(Self { url: url.to_owned(), root_cert, node })
    }

    /// A connection of its own, for blocking reads.
    pub(crate) async fn connection(&self) -> RedisResult<Conn> {
        match &self.node {
            Node::Single(client) => {
                let config = AsyncConnectionConfig::new().set_response_timeout(Some(READ_TIMEOUT));
                client.get_multiplexed_async_connection_with_config(&config).await.map(Conn::Single)
            }
            Node::Cluster(cluster) => {
                let config = ClusterConfig::new().set_response_timeout(READ_TIMEOUT);
                cluster.client.get_async_connection_with_config(config).await.map(Conn::Cluster)
            }
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
                let client = cluster_builder(&self.url, &self.root_cert).push_sender(pushes).build()?;
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

fn cluster_builder(url: &str, root_cert: &Option<Vec<u8>>) -> ClusterClientBuilder {
    let builder = ClusterClientBuilder::new([url]).use_protocol(ProtocolVersion::RESP3);
    match root_cert {
        Some(pem) => builder.certs(TlsCertificates { client_tls: None, root_cert: Some(pem.clone()) }),
        None => builder,
    }
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
    /// The connection, made in the background from the first use on.
    pub(crate) async fn get(&self) -> RedisResult<SharedConn> {
        match self {
            Self::Single(manager) => Ok(SharedConn::Single(manager.clone())),
            Self::Cluster(cluster) => cluster.shared().await.map(SharedConn::Cluster),
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
