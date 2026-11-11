//! signals-valkey: the sessions' change signals over Valkey pub/sub. Every
//! replica publishes on one channel and holds one subscriber connection on
//! it. Pub/sub keeps nothing and delivers at most once; a lost signal only
//! waits for the stream's fallback timer.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

mod budget;

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::{AsyncCommands, Client, RedisError};
use sessions::signals::{Signal, Signals, CAPACITY};
use sessions::types::SessionId;
use shared::{BackendError, WorkspaceId};
use tokio::sync::broadcast;

pub use budget::ValkeyBudget;

/// The one channel every session's signals go on.
const CHANNEL: &str = "agentd:sessions";
/// How long the subscriber waits before connecting again.
const RECONNECT: Duration = Duration::from_secs(1);
/// The longest a write waits on its signal: past it the signal is dropped
/// and the stream's fallback timer delivers.
const PUBLISH_WAIT: Duration = Duration::from_millis(200);

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
    /// Signal through the Valkey at `url` and start the subscriber. Both
    /// connect when first used, so a replica starts while Valkey is down.
    pub fn connect(url: &str) -> Result<Self, RedisError> {
        let client = Client::open(url)?;
        let publisher =
            ConnectionManager::new_lazy_with_config(client.clone(), ConnectionManagerConfig::new())?;
        let received = broadcast::channel(CAPACITY).0;
        tokio::spawn(subscribe(client, received.clone()));
        Ok(Self { publisher, received })
    }
}

#[async_trait]
impl Signals for ValkeySignals {
    async fn publish(&self, signal: &Signal) -> Result<(), BackendError> {
        let payload = format!("{} {}", signal.workspace.as_str(), signal.session);
        let mut publisher = self.publisher.clone();
        let published = tokio::time::timeout(PUBLISH_WAIT, publisher.publish::<_, _, ()>(CHANNEL, payload));
        published.await.map_err(BackendError::new)?.map_err(BackendError::new)
    }

    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.received.subscribe()
    }
}

/// Hand every signal on [`CHANNEL`] to `received`, connecting again after
/// [`RECONNECT`] whenever the connection drops.
async fn subscribe(client: Client, received: broadcast::Sender<Signal>) {
    let mut retry = tokio::time::interval(RECONNECT);
    // After a long subscription, one attempt at once, then one per RECONNECT.
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        retry.tick().await;
        let connected = client.get_async_pubsub().await;
        let Ok(mut pubsub) =
            connected.inspect_err(|e| tracing::warn!(error = %e, "signals: could not connect to Valkey"))
        else {
            continue;
        };
        let subscribed = pubsub.subscribe(CHANNEL).await;
        let Ok(()) =
            subscribed.inspect_err(|e| tracing::warn!(error = %e, "signals: could not subscribe on Valkey"))
        else {
            continue;
        };
        let mut signals = pubsub.into_on_message().filter_map(|m| {
            futures::future::ready(m.get_payload::<String>().ok().as_deref().and_then(parse))
        });
        while let Some(signal) = signals.next().await {
            let _ = received.send(signal);
        }
        tracing::warn!("signals: the Valkey subscription ended; connecting again");
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

    #[tokio::test]
    async fn with_valkey_unreachable_a_replica_starts_and_a_publish_fails_fast() {
        let signals = ValkeySignals::connect("redis://127.0.0.1:1").unwrap();
        let signal = Signal { workspace: WorkspaceId::new("wrkspc_1"), session: SessionId::generate() };
        let started = tokio::time::Instant::now();
        assert!(signals.publish(&signal).await.is_err());
        assert!(started.elapsed() < PUBLISH_WAIT * 3, "{:?}", started.elapsed());
    }

    #[tokio::test]
    async fn a_signal_published_reaches_every_subscriber() {
        let Ok(url) = std::env::var("AGENTD_VALKEY_URL") else { return };
        let (a, b) = (ValkeySignals::connect(&url).unwrap(), ValkeySignals::connect(&url).unwrap());
        let mut heard = b.subscribe();
        let signal = Signal { workspace: WorkspaceId::new("wrkspc_1"), session: SessionId::generate() };
        // The subscriber connects in the background; publish until it hears.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            a.publish(&signal).await.unwrap();
            if let Ok(Ok(got)) = tokio::time::timeout(Duration::from_millis(200), heard.recv()).await {
                assert_eq!(got, signal);
                return;
            }
            assert!(tokio::time::Instant::now() < deadline, "no signal arrived");
        }
    }
}
