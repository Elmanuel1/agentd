//! `webhook_deliveries` (V9, V24): one row per event and endpoint, sent from
//! the row by whichever replica claims it ([`queue`]), and
//! `webhook_endpoint_states` (V24), each endpoint's shared health
//! ([`endpoints`]).

use crate::PgBackend;

mod endpoints;
mod insert;
mod queue;

pub(crate) use insert::{insert_deliveries, status_changed};

/// The delivery queue and the endpoint states on Postgres.
#[derive(Debug, Clone)]
pub struct PgWebhooks {
    backend: PgBackend,
}

impl PgWebhooks {
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}
