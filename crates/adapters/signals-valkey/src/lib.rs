//! signals-valkey: the sessions' change signals over Valkey pub/sub. Every
//! replica publishes on one channel and holds one subscriber connection on
//! it. Pub/sub keeps nothing and delivers at most once; a lost signal only
//! waits for the stream's fallback timer.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

mod budget;
mod changes;
mod fires;
mod trim;
mod valkey;

use std::time::Duration;

use futures::StreamExt;
use redis::{AsyncCommands, RedisError};
use sessions::signals::{CAPACITY, NativeSignals, Signal};
use sessions::types::SessionId;
use shared::{BackendError, WorkspaceId};
use tokio::sync::broadcast;

pub use budget::ValkeyBudget;
pub use changes::ValkeyChanges;
pub use valkey::Valkey;

/// The one channel every session's signals go on.
const CHANNEL: &str = "agentd:sessions";
/// How long the subscriber waits before connecting again.
const RECONNECT: Duration = Duration::from_secs(1);
/// The longest a write waits on its signal: past it the signal is dropped
/// and the stream's fallback timer delivers.
const PUBLISH_WAIT: Duration = Duration::from_millis(200);

/// [`Signals`](sessions::signals::Signals) over Valkey.
#[derive(Clone)]
pub struct ValkeySignals {
    publisher: valkey::Shared,
    received: broadcast::Sender<Signal>,
}

impl std::fmt::Debug for ValkeySignals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValkeySignals").finish_non_exhaustive()
    }
}

impl ValkeySignals {
    /// Signal through `valkey` and start the subscriber. Both connect when
    /// first used, so a replica starts while Valkey is down.
    pub fn connect(valkey: &Valkey) -> Result<Self, RedisError> {
        let publisher = valkey.shared()?;
        let received = broadcast::channel(CAPACITY).0;
        tokio::spawn(subscribe(valkey.clone(), received.clone()));
        Ok(Self { publisher, received })
    }
}

impl NativeSignals for ValkeySignals {
    async fn publish(&self, signal: &Signal) -> Result<(), BackendError> {
        let payload = format!("{} {}", signal.workspace.as_str(), signal.session);
        let published = tokio::time::timeout(PUBLISH_WAIT, async {
            self.publisher.get().await?.publish::<_, _, ()>(CHANNEL, payload).await
        });
        published.await.map_err(BackendError::new)?.map_err(BackendError::new)
    }

    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.received.subscribe()
    }
}

/// Hand every signal on [`CHANNEL`] to `received`, connecting again after
/// [`RECONNECT`] whenever the connection drops.
async fn subscribe(valkey: Valkey, received: broadcast::Sender<Signal>) {
    let mut retry = tokio::time::interval(RECONNECT);
    // After a long subscription, one attempt at once, then one per RECONNECT.
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        retry.tick().await;
        let subscribed = valkey.subscribe(CHANNEL).await;
        let Ok(payloads) =
            subscribed.inspect_err(|e| tracing::warn!(error = %e, "signals: could not subscribe on Valkey"))
        else {
            continue;
        };
        let mut signals = payloads.filter_map(|p| futures::future::ready(parse(&p)));
        while let Some(signal) = signals.next().await {
            let _ = received.send(signal);
        }
        tracing::warn!("signals: the Valkey subscription ended; connecting again");
    }
}

/// The test Valkeys: standalone at `AGENTD_VALKEY_URL`, a cluster at
/// `AGENTD_VALKEY_CLUSTER_URL`; each only when set.
#[cfg(test)]
pub(crate) fn test_valkeys() -> Vec<Valkey> {
    let open = |var: &str, cluster| std::env::var(var).ok().map(|url| Valkey::open(&url, cluster).unwrap());
    open("AGENTD_VALKEY_URL", false).into_iter().chain(open("AGENTD_VALKEY_CLUSTER_URL", true)).collect()
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
        for cluster in [false, true] {
            let signals =
                ValkeySignals::connect(&Valkey::open("redis://127.0.0.1:1", cluster).unwrap()).unwrap();
            let signal = Signal { workspace: WorkspaceId::new("wrkspc_1"), session: SessionId::generate() };
            let started = tokio::time::Instant::now();
            assert!(signals.publish(&signal).await.is_err());
            assert!(started.elapsed() < PUBLISH_WAIT * 3, "cluster {cluster}: {:?}", started.elapsed());
        }
    }

    #[tokio::test]
    async fn a_signal_published_reaches_every_subscriber() {
        for valkey in test_valkeys() {
            heard_across(&valkey).await;
        }
    }

    async fn heard_across(valkey: &Valkey) {
        let (a, b) = (ValkeySignals::connect(valkey).unwrap(), ValkeySignals::connect(valkey).unwrap());
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
