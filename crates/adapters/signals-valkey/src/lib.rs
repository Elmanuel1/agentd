//! signals-valkey: the sessions' change signals over Valkey pub/sub, through
//! [`ValkeyBroadcast`]. Every replica publishes on one channel and holds one
//! subscriber connection on it. Pub/sub keeps nothing and delivers at most
//! once; a lost signal only waits for the stream's fallback timer.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

mod broadcast;
mod budget;
mod changes;
mod fires;
mod rate;
mod trim;
mod valkey;

use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use redis::RedisError;
use sessions::signals::{CAPACITY, NativeSignals, Signal};
use sessions::types::SessionId;
use shared::ports::broadcast::NativeBroadcast;
use shared::{BackendError, WorkspaceId};
use tokio::sync::broadcast as channel;

pub use broadcast::ValkeyBroadcast;
pub use budget::ValkeyBudget;
pub use changes::ValkeyChanges;
pub use rate::ValkeyRateCounter;
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
    broadcast: ValkeyBroadcast,
    received: channel::Sender<Signal>,
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
        let broadcast = ValkeyBroadcast::connect(valkey)?;
        let received = channel::channel(CAPACITY).0;
        tokio::spawn(relay(NativeBroadcast::subscribe(&broadcast, CHANNEL), received.clone()));
        Ok(Self { broadcast, received })
    }
}

impl NativeSignals for ValkeySignals {
    async fn publish(&self, signal: &Signal) -> Result<(), BackendError> {
        let payload = format!("{} {}", signal.workspace.as_str(), signal.session);
        NativeBroadcast::publish(&self.broadcast, CHANNEL, &payload).await
    }

    fn subscribe(&self) -> channel::Receiver<Signal> {
        self.received.subscribe()
    }
}

/// Hand every signal on [`CHANNEL`] to `received`.
async fn relay(payloads: BoxStream<'static, String>, received: channel::Sender<Signal>) {
    let mut signals = payloads.filter_map(|p| futures::future::ready(parse(&p)));
    while let Some(signal) = signals.next().await {
        let _ = received.send(signal);
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

    /// `rediss://` at `AGENTD_VALKEY_TLS_URL`, whose certificate the CA at
    /// `AGENTD_VALKEY_TLS_CA` signs (`docker/valkey/start-tls.sh`).
    #[tokio::test]
    async fn rediss_trusts_the_ca_it_is_given_and_not_otherwise() {
        let (Ok(url), Ok(ca)) =
            (std::env::var("AGENTD_VALKEY_TLS_URL"), std::env::var("AGENTD_VALKEY_TLS_CA"))
        else {
            return;
        };
        let ping = |valkey: Valkey| async move {
            let mut conn = valkey.connection().await?;
            redis::cmd("PING").query_async::<String>(&mut conn).await
        };
        assert_eq!(ping(Valkey::open_trusting(&url, false, Some(&ca)).unwrap()).await.unwrap(), "PONG");
        let untrusted = ping(Valkey::open(&url, false).unwrap()).await.unwrap_err();
        assert!(untrusted.to_string().contains("UnknownIssuer"), "{untrusted}");
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
