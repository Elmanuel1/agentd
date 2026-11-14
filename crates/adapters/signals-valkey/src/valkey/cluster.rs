//! A cluster's client and the connection its short commands share. The
//! shared connection is made by a task of its own, retrying until it is made,
//! so a caller that gives up waiting does not abandon a connection half made.

use std::sync::{Arc, Once, OnceLock};
use std::time::Duration;

use redis::cluster::ClusterClient;
use redis::cluster_async::ClusterConnection;
use redis::{ErrorKind, RedisError, RedisResult};
use tokio::sync::Notify;

/// The pause before another try at the shared connection.
const RETRY: Duration = Duration::from_secs(1);

/// A cluster's client, and the connection its short commands share.
pub(crate) struct Cluster {
    pub(super) client: ClusterClient,
    shared: OnceLock<ClusterConnection>,
    connecting: Once,
    made: Notify,
}

impl Cluster {
    pub(super) fn new(client: ClusterClient) -> Self {
        Self { client, shared: OnceLock::new(), connecting: Once::new(), made: Notify::new() }
    }

    /// The shared connection, waiting until it is made; bound the wait with a timeout.
    pub(super) async fn shared(self: &Arc<Self>) -> RedisResult<ClusterConnection> {
        if let Some(conn) = self.shared.get() {
            return Ok(conn.clone());
        }
        self.connecting.call_once(|| drop(tokio::spawn(self.clone().connect())));
        let made = self.made.notified();
        tokio::pin!(made);
        made.as_mut().enable();
        if self.shared.get().is_none() {
            made.await;
        }
        let conn = self.shared.get().cloned();
        conn.ok_or_else(|| RedisError::from((ErrorKind::Io, "the cluster connection was not made")))
    }

    async fn connect(self: Arc<Self>) {
        loop {
            match self.client.get_async_connection().await {
                Ok(conn) => {
                    let _ = self.shared.set(conn);
                    self.made.notify_waiters();
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "valkey: could not connect to the cluster; trying again");
                    tokio::time::sleep(RETRY).await;
                }
            }
        }
    }
}
