//! `SessionsStore` on Postgres via Diesel: the session rows.

use ::sessions::store::{SessionsStore, StoreError};
use ::sessions::types::{Session, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::{Pagination, WorkspaceId};

use crate::schema::sessions;
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
                workspace_id: ws.as_str().to_owned(),
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
            .filter(sessions::workspace_id.eq(ws.as_str()))
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
                .filter(sessions::workspace_id.eq(ws.as_str()))
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
        // Delete + return the row in one statement (no separate get).
        let doc: Option<serde_json::Value> = diesel::delete(
            sessions::table
                .filter(sessions::workspace_id.eq(ws.as_str()))
                .filter(sessions::session_id.eq(id.as_str())),
        )
        .returning(sessions::doc)
        .get_result(&mut conn)
        .await
        .optional()
        .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::NotFound)?)
    }

    async fn list(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Session>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = sessions::table
            .filter(sessions::workspace_id.eq(ws.as_str()))
            .order(sessions::created_at.asc())
            .select(sessions::doc)
            .limit(page.fetch_limit())
            .offset(page.sql_offset())
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }
}
