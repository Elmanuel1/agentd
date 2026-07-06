//! `SessionsStore` on Postgres via Diesel — session rows + the shared
//! append-only event log (SSE tails `session_events` by `seq`).

use agentd_core::WorkspaceId;
use agentd_sessions::events::Event;
use agentd_sessions::store::{SessionsStore, StoreError};
use agentd_sessions::types::{Session, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;

use crate::schema::{session_events, sessions};
use crate::{diesel_be, enum_str, parse_doc, pool_be, to_doc, PgBackend};

/// Postgres-backed sessions store (Diesel).
#[derive(Debug, Clone)]
pub struct PgSessionsStore {
    backend: PgBackend,
}

impl PgSessionsStore {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }

    async fn require_session(&self, ws: &WorkspaceId, id: &SessionId) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let exists: Option<String> = sessions::table
            .filter(sessions::workspace_id.eq(&ws.0))
            .filter(sessions::session_id.eq(id.as_str()))
            .select(sessions::session_id)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        exists.map(|_| ()).ok_or(StoreError::NotFound)
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = sessions)]
struct NewSession {
    workspace_id: String,
    session_id: String,
    status: String,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

#[async_trait]
impl SessionsStore for PgSessionsStore {
    async fn insert(&self, ws: &WorkspaceId, s: Session) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(sessions::table)
            .values(NewSession {
                workspace_id: ws.0.clone(),
                session_id: s.id.as_str().to_owned(),
                status: enum_str(&s.status),
                created_at: s.created_at.as_datetime(),
                doc: to_doc(&s)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = sessions::table
            .filter(sessions::workspace_id.eq(&ws.0))
            .filter(sessions::session_id.eq(id.as_str()))
            .select(sessions::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::NotFound)?)
    }

    async fn replace(&self, ws: &WorkspaceId, s: Session) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let affected = diesel::update(
            sessions::table
                .filter(sessions::workspace_id.eq(&ws.0))
                .filter(sessions::session_id.eq(s.id.as_str())),
        )
        .set((sessions::status.eq(enum_str(&s.status)), sessions::doc.eq(to_doc(&s)?)))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn delete(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        // Log rows first so a crash never orphans events.
        diesel::delete(
            session_events::table
                .filter(session_events::workspace_id.eq(&ws.0))
                .filter(session_events::session_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        // Delete + return the row in one statement (no separate get).
        let doc: Option<serde_json::Value> = diesel::delete(
            sessions::table
                .filter(sessions::workspace_id.eq(&ws.0))
                .filter(sessions::session_id.eq(id.as_str())),
        )
        .returning(sessions::doc)
        .get_result(&mut conn)
        .await
        .optional()
        .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::NotFound)?)
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Session>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = sessions::table
            .filter(sessions::workspace_id.eq(&ws.0))
            .order(sessions::created_at.asc())
            .select(sessions::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }

    async fn append_event(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        event: Event,
    ) -> Result<(), StoreError> {
        self.require_session(ws, id).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        // DB stamps the log time (authoritative ordering is `seq` bigserial).
        diesel::insert_into(session_events::table)
            .values((
                session_events::workspace_id.eq(&ws.0),
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

    async fn replace_event(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let affected = diesel::update(
            session_events::table
                .filter(session_events::workspace_id.eq(&ws.0))
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
            .filter(session_events::workspace_id.eq(&ws.0))
            .filter(session_events::session_id.eq(id.as_str()))
            .order(session_events::seq.asc())
            .select(session_events::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }
}
