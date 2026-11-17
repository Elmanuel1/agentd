//! What adapters implement for the ports of the parent module; each comes with its `dyn` trait.

use super::*;

/// What an adapter implements to provide [`Consumer`]; the methods are documented there.
pub(crate) trait NativeConsumer: Send + Sync {
    fn ordered(&self) -> bool;
    fn consume(&self, entry: &StreamId) -> impl Future<Output = bool> + Send;
}

impl<T: NativeConsumer> Consumer for T {
    fn ordered(&self) -> bool {
        NativeConsumer::ordered(self)
    }
    fn consume<'a>(&'a self, entry: &'a StreamId) -> BoxFuture<'a, bool> {
        Box::pin(NativeConsumer::consume(self, entry))
    }
}
