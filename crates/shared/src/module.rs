//! A resource's contribution to the app: its routes and its fire handlers.
//! The binary assembles one [`Module`] per resource.

use std::sync::Arc;

use axum::{Extension, Router};

use crate::ports::fire::FireHandler;

/// One resource's routes and fire handlers.
pub struct Module {
    /// The resource's routes, with its service bound as an `Extension`.
    pub router: Router,
    /// The handlers for the resource's scheduled work.
    pub fire_handlers: Vec<Arc<dyn FireHandler>>,
}

impl std::fmt::Debug for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Module").field("fire_handlers", &self.fire_handlers.len()).finish_non_exhaustive()
    }
}

impl Module {
    /// `router` with `service` available to its handlers.
    pub fn new<S: Clone + Send + Sync + 'static>(router: Router, service: S) -> Self {
        Self { router: router.layer(Extension(service)), fire_handlers: Vec::new() }
    }

    /// Add a handler for the resource's scheduled work.
    #[must_use]
    pub fn with_fire_handler(mut self, handler: Arc<dyn FireHandler>) -> Self {
        self.fire_handlers.push(handler);
        self
    }
}
