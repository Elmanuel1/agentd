//! The webhook sender's wiring (ADR 0001): the change feed's handler that
//! wakes every instance on a new delivery, and the claim loop each instance
//! runs until shutdown.

use std::sync::Arc;

use shared::net::Guard;
use shared::ports::broadcast::Broadcast;
use shared::ports::changes::ChangeHandler;
use store_postgres::{PgBackend, PgWebhooks};
use webhooks::{DeliveryWake, SenderMetrics, SenderPorts, WebhookSender, Webhooks};

use crate::shutdown::Shutdown;

/// Start this instance's claim loop; the handler the change feed hands new deliveries to.
pub(super) fn start(
    hooks: Arc<Webhooks>,
    pg: &PgBackend,
    wakes: Arc<dyn Broadcast>,
    metrics: Arc<dyn SenderMetrics>,
    shutdown: &mut Shutdown,
) -> Arc<dyn ChangeHandler> {
    let store = Arc::new(PgWebhooks::new(pg.clone()));
    let ports = SenderPorts { queue: store.clone(), states: store, wakes: wakes.clone(), metrics };
    let sender = WebhookSender::new(hooks, &Guard::default(), ports);
    shutdown.drain(tokio::spawn(sender.run(shutdown.signal())));
    Arc::new(DeliveryWake::new(wakes))
}
