//! signals-valkey: the sessions' change signals over Valkey pub/sub. Every
//! replica publishes on one channel and holds one subscriber connection on
//! it. Pub/sub keeps nothing and delivers at most once; a lost signal only
//! waits for the stream's fallback timer.

#![cfg_attr(test, allow(clippy::unwrap_used))]

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client, RedisError};
use sessions::signals::{Signal, Signals, CAPACITY};
use sessions::types::SessionId;
use shared::{BackendError, WorkspaceId};
use tokio::sync::broadcast;

/// The one channel every session's signals go on.
const CHANNEL: &str = "agentd:sessions";
/// How long the subscriber waits before connecting again.
const RECONNECT: Duration = Duration::from_secs(1);

/// [`Signals`] over Valkey.
#[derive(Clone)]
pub struct ValkeySignals {
    publisher: ConnectionManager,
    received: broadcast::Sender<Signal>,
}

impl std::fmt::Debug for ValkeySignals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValkeySignals").finish_non_exhaustive()
    }
}

impl ValkeySignals {
    /// Connect to the Valkey at `url` and start the subscriber.
    pub async fn connect(url: &str) -> Result<Self, RedisError> {
        let client = Client::open(url)?;
        let publisher = ConnectionManager::new(client.clone()).await?;
        let received = broadcast::channel(CAPACITY).0;
        tokio::spawn(subscribe(client, received.clone()));
        Ok(Self { publisher, received })
    }
}

#[async_trait]
impl Signals for ValkeySignals {
    async fn publish(&self, signal: &Signal) -> Result<(), BackendError> {
        let payload = format!("{} {}", signal.workspace.as_str(), signal.session);
        self.publisher.clone().publish::<_, _, ()>(CHANNEL, payload).await.map_err(BackendError::new)
    }

    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.received.subscribe()
    }
}

/// Hand every signal on [`CHANNEL`] to `received`, connecting again after
/// [`RECONNECT`] whenever the connection drops.
async fn subscribe(client: Client, received: broadcast::Sender<Signal>) {
    loop {
        match client.get_async_pubsub().await {
            Ok(mut pubsub) => match pubsub.subscribe(CHANNEL).await {
                Ok(()) => {
                    let mut messages = pubsub.into_on_message();
                    while let Some(message) = messages.next().await {
                        if let Some(signal) = message.get_payload::<String>().ok().as_deref().and_then(parse)
                        {
                            let _ = received.send(signal);
                        }
                    }
                    tracing::warn!("signals: the Valkey subscription ended; connecting again");
                }
                Err(e) => tracing::warn!(error = %e, "signals: could not subscribe on Valkey"),
            },
            Err(e) => tracing::warn!(error = %e, "signals: could not connect to Valkey"),
        }
        tokio::time::sleep(RECONNECT).await;
    }
}

fn parse(payload: &str) -> Option<Signal> {
    let (workspace, session) = payload.split_once(' ')?;
    Some(Signal { workspace: WorkspaceId::new(workspace), session: SessionId::parse(session).ok()? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_names_its_workspace_and_session() {
        let session = SessionId::generate();
        let signal = parse(&format!("wrkspc_1 {session}")).unwrap();
        assert_eq!(signal, Signal { workspace: WorkspaceId::new("wrkspc_1"), session });
        assert_eq!(parse("no-space"), None);
        assert_eq!(parse("wrkspc_1 not-a-session"), None);
    }
}
