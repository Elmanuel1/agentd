//! `SessionEventLog` on Postgres via Diesel: the shared append-only event log
//! (SSE tails `session_events` by `seq`).

use ::sessions::events::Event;
use ::sessions::store::{SessionEventLog, StoreError};
use ::sessions::types::SessionId;
use async_trait::async_trait;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::WorkspaceId;

use crate::schema::{session_events, sessions};
use crate::{diesel_be, parse_doc, pool_be, to_doc, PgBackend};

/// Postgres-backed session event log (Diesel).
#[derive(Debug, Clone)]
pub struct PgSessionEventLog {
    backend: PgBackend,
}

impl PgSessionEventLog {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }

    async fn require_session(&self, ws: &WorkspaceId, id: &SessionId) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let exists: Option<String> = sessions::table
            .filter(sessions::workspace_id.eq(ws.as_str()))
            .filter(sessions::session_id.eq(id.as_str()))
            .select(sessions::session_id)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        exists.map(|_| ()).ok_or(StoreError::NotFound)
    }
}

#[async_trait]
impl SessionEventLog for PgSessionEventLog {
    async fn append_event(&self, ws: &WorkspaceId, id: &SessionId, event: Event) -> Result<(), StoreError> {
        self.require_session(ws, id).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        // DB stamps the log time (authoritative ordering is `seq` bigserial).
        diesel::insert_into(session_events::table)
            .values((
                session_events::workspace_id.eq(ws.as_str()),
                session_events::session_id.eq(id.as_str()),
                session_events::event_id.eq(event.id.as_str()),
                session_events::created_at.eq(diesel::dsl::now),
                session_events::doc.eq(to_doc(&event)?),
            ))
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn replace_event(&self, ws: &WorkspaceId, id: &SessionId, event: Event) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let affected = diesel::update(
            session_events::table
                .filter(session_events::workspace_id.eq(ws.as_str()))
                .filter(session_events::session_id.eq(id.as_str()))
                .filter(session_events::event_id.eq(event.id.as_str())),
        )
        .set(session_events::doc.eq(to_doc(&event)?))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Vec<Event>, StoreError> {
        self.require_session(ws, id).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = session_events::table
            .filter(session_events::workspace_id.eq(ws.as_str()))
            .filter(session_events::session_id.eq(id.as_str()))
            .order(session_events::seq.asc())
            .select(session_events::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }

    async fn delete_events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::delete(
            session_events::table
                .filter(session_events::workspace_id.eq(ws.as_str()))
                .filter(session_events::session_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(())
    }
}
