//! Session persistence: session rows + the append-only event log.

use std::collections::BTreeMap;

use agentd_core::WorkspaceId;
use async_trait::async_trait;
use parking_lot::RwLock;

use crate::events::Event;
use crate::types::{Session, SessionId};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such session.
    #[error("session not found")]
    NotFound,
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
    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Session>, StoreError>;
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

type Entry = (Session, Vec<Event>);

/// In-memory store.
#[derive(Debug, Default)]
pub struct InMemorySessionsStore {
    inner: RwLock<BTreeMap<WorkspaceId, BTreeMap<SessionId, Entry>>>,
}

impl InMemorySessionsStore {
    /// New empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionsStore for InMemorySessionsStore {
    async fn insert(&self, ws: &WorkspaceId, s: Session) -> Result<(), StoreError> {
        self.inner
            .write()
            .entry(ws.clone())
            .or_default()
            .insert(s.id.clone(), (s, Vec::new()));
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError> {
        self.inner
            .read()
            .get(ws)
            .and_then(|m| m.get(id))
            .map(|(s, _)| s.clone())
            .ok_or(StoreError::NotFound)
    }

    async fn replace(&self, ws: &WorkspaceId, s: Session) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let e = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(&s.id))
            .ok_or(StoreError::NotFound)?;
        e.0 = s;
        Ok(())
    }

    async fn delete(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError> {
        self.inner
            .write()
            .get_mut(ws)
            .and_then(|m| m.remove(id))
            .map(|(s, _)| s)
            .ok_or(StoreError::NotFound)
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Session>, StoreError> {
        Ok(self
            .inner
            .read()
            .get(ws)
            .map(|m| m.values().map(|(s, _)| s.clone()).collect())
            .unwrap_or_default())
    }

    async fn append_event(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        event: Event,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let e = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(id))
            .ok_or(StoreError::NotFound)?;
        e.1.push(event);
        Ok(())
    }

    async fn replace_event(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        event: Event,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let e = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(id))
            .ok_or(StoreError::NotFound)?;
        for slot in &mut e.1 {
            if slot.id == event.id {
                *slot = event;
                return Ok(());
            }
        }
        Err(StoreError::NotFound)
    }

    async fn events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Vec<Event>, StoreError> {
        self.inner
            .read()
            .get(ws)
            .and_then(|m| m.get(id))
            .map(|(_, ev)| ev.clone())
            .ok_or(StoreError::NotFound)
    }
}
