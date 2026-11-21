//! [`Broadcast`](shared::ports::broadcast::Broadcast) over Valkey pub/sub:
//! each `subscribe` holds one subscriber connection of its own, connecting
//! again a second after it drops; a publish waits at most 200 ms.

use futures::StreamExt;
use futures::stream::BoxStream;
use redis::{AsyncCommands, RedisError};
use shared::BackendError;
use shared::ports::broadcast::NativeBroadcast;

use crate::valkey::{Shared, Valkey};
use crate::{PUBLISH_WAIT, RECONNECT};

/// Payloads a slow listener may fall behind by before the newest are dropped.
const BUFFER: usize = 1_024;

/// Messages between replicas over one Valkey.
#[derive(Clone)]
pub struct ValkeyBroadcast {
    valkey: Valkey,
    publisher: Shared,
}

impl std::fmt::Debug for ValkeyBroadcast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValkeyBroadcast").finish_non_exhaustive()
    }
}

impl ValkeyBroadcast {
    /// Over `valkey`; nothing connects until first used.
    pub fn connect(valkey: &Valkey) -> Result<Self, RedisError> {
        Ok(Self { valkey: valkey.clone(), publisher: valkey.shared()? })
    }
}

impl NativeBroadcast for ValkeyBroadcast {
    async fn publish(&self, channel: &str, payload: &str) -> Result<(), BackendError> {
        let published = tokio::time::timeout(PUBLISH_WAIT, async {
            self.publisher.get().await?.publish::<_, _, ()>(channel, payload).await
        });
        published.await.map_err(BackendError::new)?.map_err(BackendError::new)
    }

    fn subscribe(&self, channel: &str) -> BoxStream<'static, String> {
        let (heard, payloads) = tokio::sync::mpsc::channel(BUFFER);
        tokio::spawn(listen(self.valkey.clone(), channel.to_owned(), heard));
        futures::stream::unfold(payloads, |mut p| async move { p.recv().await.map(|m| (m, p)) }).boxed()
    }
}

/// Hand every payload on `channel` to `heard` until it is dropped, connecting again after [`RECONNECT`].
async fn listen(valkey: Valkey, channel: String, heard: tokio::sync::mpsc::Sender<String>) {
    let mut retry = tokio::time::interval(RECONNECT);
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    while !heard.is_closed() {
        retry.tick().await;
        let Ok(mut payloads) = valkey.subscribe(&channel).await.inspect_err(
            |e| tracing::warn!(%channel, error = %e, "broadcast: could not subscribe on Valkey"),
        ) else {
            continue;
        };
        while let Some(payload) = payloads.next().await {
            if heard.try_send(payload).is_err() && heard.is_closed() {
                return;
            }
        }
        tracing::warn!(%channel, "broadcast: the Valkey subscription ended; connecting again");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use shared::ports::broadcast::Broadcast;

    use super::*;

    #[tokio::test]
    async fn a_message_published_reaches_every_replica() {
        for valkey in crate::test_valkeys() {
            let (a, b) =
                (ValkeyBroadcast::connect(&valkey).unwrap(), ValkeyBroadcast::connect(&valkey).unwrap());
            let channel = format!("agentd:test:{}", uuid::Uuid::new_v4());
            let mut heard = Broadcast::subscribe(&b, &channel);
            // The subscriber connects in the background; publish until it hears.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                Broadcast::publish(&a, &channel, "wake").await.unwrap();
                if let Ok(got) = tokio::time::timeout(Duration::from_millis(200), heard.next()).await {
                    assert_eq!(got.as_deref(), Some("wake"));
                    break;
                }
                assert!(tokio::time::Instant::now() < deadline, "no message arrived");
            }
        }
    }
}
