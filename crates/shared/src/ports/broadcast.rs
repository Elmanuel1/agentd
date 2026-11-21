//! Messages every replica hears: a webhook wake (`agentd:webhooks`), a key
//! revoke (`agentd:keys`). Delivery is at most once and nothing is kept, so a
//! listener always has a slower path that does not need the message.

use std::future::Future;

use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::broadcast;

use crate::BackendError;
use crate::ports::BoxFuture;

/// Publish and hear messages on named channels.
pub trait Broadcast: Send + Sync {
    /// Send `payload` to every replica listening on `channel`.
    fn publish<'a>(&'a self, channel: &'a str, payload: &'a str) -> BoxFuture<'a, Result<(), BackendError>>;
    /// The payloads published on `channel` from now on; the stream reconnects by itself and never ends.
    fn subscribe(&self, channel: &str) -> BoxStream<'static, String>;
}

/// What an adapter implements to provide [`Broadcast`]; the methods are documented there.
pub trait NativeBroadcast: Send + Sync {
    fn publish(&self, channel: &str, payload: &str) -> impl Future<Output = Result<(), BackendError>> + Send;
    fn subscribe(&self, channel: &str) -> BoxStream<'static, String>;
}

impl<T: NativeBroadcast> Broadcast for T {
    fn publish<'a>(&'a self, channel: &'a str, payload: &'a str) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(NativeBroadcast::publish(self, channel, payload))
    }
    fn subscribe(&self, channel: &str) -> BoxStream<'static, String> {
        NativeBroadcast::subscribe(self, channel)
    }
}

/// [`Broadcast`] inside one process, for a single replica and for tests.
#[derive(Debug, Clone)]
pub struct LocalBroadcast {
    sent: broadcast::Sender<(String, String)>,
}

impl Default for LocalBroadcast {
    fn default() -> Self {
        Self { sent: broadcast::channel(1_024).0 }
    }
}

impl NativeBroadcast for LocalBroadcast {
    async fn publish(&self, channel: &str, payload: &str) -> Result<(), BackendError> {
        let _ = self.sent.send((channel.to_owned(), payload.to_owned()));
        Ok(())
    }

    fn subscribe(&self, channel: &str) -> BoxStream<'static, String> {
        let channel = channel.to_owned();
        received(self.sent.subscribe())
            .filter_map(move |(on, payload)| futures::future::ready((on == channel).then_some(payload)))
            .boxed()
    }
}

/// Every message `rx` receives, skipping over any it lagged behind on.
fn received<T: Clone + Send + 'static>(rx: broadcast::Receiver<T>) -> impl futures::Stream<Item = T> {
    futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(item) => return Some((item, rx)),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_listener_hears_only_its_channel() {
        let local = LocalBroadcast::default();
        let mut keys = Broadcast::subscribe(&local, "agentd:keys");
        Broadcast::publish(&local, "agentd:webhooks", "wake").await.unwrap();
        Broadcast::publish(&local, "agentd:keys", "key_1").await.unwrap();
        assert_eq!(keys.next().await.as_deref(), Some("key_1"));
    }
}
