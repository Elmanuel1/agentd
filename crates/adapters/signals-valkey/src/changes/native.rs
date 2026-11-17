//! What adapters implement for the ports of the parent module; each comes with its `dyn` trait.

use super::*;

/// What an adapter implements to provide [`Consumer`]; the methods are documented there.
pub(crate) trait ConsumerImpl: Send + Sync {
    fn ordered(&self) -> bool;
    fn consume(&self, entry: &StreamId) -> impl Future<Output = bool> + Send;
}

impl<T: ConsumerImpl> Consumer for T {
    fn ordered(&self) -> bool {
        ConsumerImpl::ordered(self)
    }
    fn consume<'a>(&'a self, entry: &'a StreamId) -> BoxFuture<'a, bool> {
        Box::pin(ConsumerImpl::consume(self, entry))
    }
}
