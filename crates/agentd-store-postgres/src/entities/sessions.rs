//! `SessionsStore` on Postgres — session rows + the shared append-only event
//! log (SSE tails `session_events` by `seq`, so any replica serves the stream).

use agentd_core::WorkspaceId;
use agentd_sessions::events::Event;
use agentd_sessions::store::{SessionsStore, StoreError};
use agentd_sessions::types::{Session, SessionId};
use async_trait::async_trait;
use sqlx::types::Json;

use crate::{enum_str, sqlx_msg, PgBackend};

/// Postgres-backed sessions store.
#[derive(Debug, Clone)]
pub struct PgSessionsStore {
    backend: PgBackend,
}

impl PgSessionsStore {
    /// Wrap a pool.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }

    async fn require_session(&self, ws: &WorkspaceId, id: &SessionId) -> Result<(), StoreError> {
        let exists: Option<(i32,)> =
            sqlx::query_as("select 1 from sessions where workspace_id = $1 and session_id = $2")
                .bind(&ws.0)
                .bind(id.as_str())
                .fetch_optional(&self.backend.pool)
                .await
                .map_err(be)?;
        exists.map(|_| ()).ok_or(StoreError::NotFound)
    }
}

fn be(e: sqlx::Error) -> StoreError {
    StoreError::Backend(sqlx_msg(e))
}

#[async_trait]
impl SessionsStore for PgSessionsStore {
    async fn insert(&self, ws: &WorkspaceId, s: Session) -> Result<(), StoreError> {
        sqlx::query(
            "insert into sessions (workspace_id, session_id, status, created_at, doc)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(&ws.0)
        .bind(s.id.as_str())
        .bind(enum_str(&s.status))
        .bind(s.created_at.as_datetime())
        .bind(Json(&s))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError> {
        let row: Option<(Json<Session>,)> =
            sqlx::query_as("select doc from sessions where workspace_id = $1 and session_id = $2")
                .bind(&ws.0)
                .bind(id.as_str())
                .fetch_optional(&self.backend.pool)
                .await
                .map_err(be)?;
        row.map(|(d,)| d.0).ok_or(StoreError::NotFound)
    }

    async fn replace(&self, ws: &WorkspaceId, s: Session) -> Result<(), StoreError> {
        let affected = sqlx::query(
            "update sessions set status = $3, doc = $4
             where workspace_id = $1 and session_id = $2",
        )
        .bind(&ws.0)
        .bind(s.id.as_str())
        .bind(enum_str(&s.status))
        .bind(Json(&s))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn delete(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError> {
        // Log rows go first so a crash mid-delete never orphans events.
        sqlx::query("delete from session_events where workspace_id = $1 and session_id = $2")
            .bind(&ws.0)
            .bind(id.as_str())
            .execute(&self.backend.pool)
            .await
            .map_err(be)?;
        let row: Option<(Json<Session>,)> = sqlx::query_as(
            "delete from sessions where workspace_id = $1 and session_id = $2 returning doc",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        row.map(|(d,)| d.0).ok_or(StoreError::NotFound)
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Session>, StoreError> {
        let rows: Vec<(Json<Session>,)> = sqlx::query_as(
            "select doc from sessions where workspace_id = $1 order by created_at",
        )
        .bind(&ws.0)
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }

    async fn append_event(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        event: Event,
    ) -> Result<(), StoreError> {
        self.require_session(ws, id).await?;
        sqlx::query(
            "insert into session_events (workspace_id, session_id, event_id, created_at, doc)
             values ($1, $2, $3, now(), $4)",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .bind(event.id.as_str())
        .bind(Json(&event))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn replace_event(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        event: Event,
    ) -> Result<(), StoreError> {
        let affected = sqlx::query(
            "update session_events set doc = $4
             where workspace_id = $1 and session_id = $2 and event_id = $3",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .bind(event.id.as_str())
        .bind(Json(&event))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Vec<Event>, StoreError> {
        self.require_session(ws, id).await?;
        let rows: Vec<(Json<Event>,)> = sqlx::query_as(
            "select doc from session_events where workspace_id = $1 and session_id = $2
             order by seq",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }
}
