//! A resource's contribution to the app: its routes and its fire handlers.
//! The binary assembles one [`Module`] per resource.

use std::sync::Arc;

use axum::{Extension, Router};

use crate::beta::BetaGate;
use crate::ports::fire::FireHandler;

/// A request body cap, and the API name its 413 message gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLimit {
    /// The API name in the 413 message (`Skills`).
    pub api: &'static str,
    /// The cap, in MB.
    pub max_mb: usize,
}

impl BodyLimit {
    /// The cap in bytes.
    #[must_use]
    pub const fn bytes(self) -> usize {
        self.max_mb * 1024 * 1024
    }
}

/// One resource's routes, fire handlers, betas and body cap.
pub struct Module {
    /// The resource's routes, with its service bound as an `Extension`.
    pub router: Router,
    /// The handlers for the resource's scheduled work.
    pub fire_handlers: Vec<Arc<dyn FireHandler>>,
    /// The betas its routes sit behind.
    pub betas: Vec<BetaGate>,
    /// Its request body cap: 32 MB unless the resource sets one.
    pub body_limit: BodyLimit,
}

impl std::fmt::Debug for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Module").field("fire_handlers", &self.fire_handlers.len()).finish_non_exhaustive()
    }
}

impl Module {
    /// `router` with `service` available to its handlers.
    pub fn new<S: Clone + Send + Sync + 'static>(router: Router, service: S) -> Self {
        Self {
            router: router.layer(Extension(service)),
            fire_handlers: Vec::new(),
            betas: Vec::new(),
            body_limit: BodyLimit { api: "Managed Agents", max_mb: 32 },
        }
    }

    /// Serve the routes behind `gate`.
    #[must_use]
    pub fn with_beta(mut self, gate: BetaGate) -> Self {
        self.betas.push(gate);
        self
    }

    /// Cap request bodies at `limit`.
    #[must_use]
    pub fn with_body_limit(mut self, limit: BodyLimit) -> Self {
        self.body_limit = limit;
        self
    }

    /// Add a handler for the resource's scheduled work.
    #[must_use]
    pub fn with_fire_handler(mut self, handler: Arc<dyn FireHandler>) -> Self {
        self.fire_handlers.push(handler);
        self
    }
}
