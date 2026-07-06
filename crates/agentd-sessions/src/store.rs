//! Session persistence: session rows + the append-only event log.

use agentd_core::{Pagination, WorkspaceId};
use async_trait::async_trait;

use crate::events::Event;
use crate::types::{Session, SessionId};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such session.
    #[error("session not found")]
    NotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(String),
}

/// Persistence port.
#[async_trait]
pub trait SessionsStore: Send + Sync {
    /// Insert a session.
    async fn insert(&self, ws: &WorkspaceId, s: Session) -> Result<(), StoreError>;
    /// Fetch a session.
    async fn get(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError>;
    /// Replace a session.
    async fn replace(&self, ws: &WorkspaceId, s: Session) -> Result<(), StoreError>;
    /// Delete a session and its event log.
    async fn delete(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError>;
    /// All sessions, insertion order.
    async fn list(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Session>, StoreError>;
    /// Append an event to the log.
    async fn append_event(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        event: Event,
    ) -> Result<(), StoreError>;
    /// Replace an event in place (processed_at stamping).
    async fn replace_event(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        event: Event,
    ) -> Result<(), StoreError>;
    /// Full event history, append order.
    async fn events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Vec<Event>, StoreError>;
}
