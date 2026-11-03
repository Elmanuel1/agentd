//! `SessionEventLog` on Postgres via Diesel: the shared append-only event log
//! (SSE tails `session_events` by `seq`).

use ::sessions::events::Event;
use ::sessions::store::{EventFilter, Order, SessionEventLog, StoreError};
use ::sessions::types::SessionId;
use async_trait::async_trait;
use diesel::sql_types::{Array, BigInt, Jsonb, Nullable, Text, Timestamptz};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::{Pagination, WorkspaceId};

use crate::schema::{session_events, sessions};
use crate::{diesel_be, parse_doc, pool_be, to_doc, DocRow, PgBackend};

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
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        // One statement: the event lands only while its session exists, so a
        // delete that removed the session first leaves no orphan behind.
        let inserted = diesel::sql_query(
            "INSERT INTO session_events (workspace_id, session_id, event_id, created_at, doc)
             SELECT $1, $2, $3, now(), $4
             WHERE EXISTS (SELECT 1 FROM sessions WHERE workspace_id = $1 AND session_id = $2)",
        )
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<Text, _>(event.id.as_str())
        .bind::<Jsonb, _>(to_doc(&event)?)
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if inserted == 0 {
            return Err(StoreError::NotFound);
        }
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

    async fn unprocessed_events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Vec<Event>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<DocRow> = diesel::sql_query(
            "SELECT doc FROM session_events
             WHERE workspace_id = $1 AND session_id = $2 AND doc->>'processed_at' IS NULL
               AND (doc->>'type' LIKE 'user.%' OR doc->>'type' = 'system.message')
             ORDER BY seq",
        )
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        rows.into_iter().map(|r| parse_doc(r.doc)).collect()
    }

    async fn events_page(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        filter: &EventFilter,
        page: &Pagination,
    ) -> Result<Vec<Event>, StoreError> {
        self.require_session(ws, id).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let p = &filter.processed;
        let order = if filter.order == Order::Desc { "DESC" } else { "ASC" };
        let processed = "(doc->>'processed_at')::timestamptz";
        let rows: Vec<DocRow> = diesel::sql_query(format!(
            "SELECT doc FROM session_events
             WHERE workspace_id = $1 AND session_id = $2
               AND (cardinality($3::text[]) = 0 OR doc->>'type' = ANY($3))
               AND ($4::timestamptz IS NULL OR {processed} > $4)
               AND ($5::timestamptz IS NULL OR {processed} >= $5)
               AND ($6::timestamptz IS NULL OR {processed} < $6)
               AND ($7::timestamptz IS NULL OR {processed} <= $7)
               AND (CASE WHEN $10::text IS NULL THEN
                      doc->>'session_thread_id' IS NULL OR doc->>'type' LIKE 'session.%'
                      OR doc->>'evaluated_permission' = 'ask' OR doc->>'type' = 'agent.custom_tool_use'
                    ELSE doc->>'session_thread_id' = $10 AND doc->>'type' <> 'session.thread_created' END)
             ORDER BY {processed} {order} NULLS LAST, seq {order}
             LIMIT $8 OFFSET $9"
        ))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<Array<Text>, _>(&filter.types)
        .bind::<Nullable<Timestamptz>, _>(p.gt.map(|t| t.as_datetime()))
        .bind::<Nullable<Timestamptz>, _>(p.gte.map(|t| t.as_datetime()))
        .bind::<Nullable<Timestamptz>, _>(p.lt.map(|t| t.as_datetime()))
        .bind::<Nullable<Timestamptz>, _>(p.lte.map(|t| t.as_datetime()))
        .bind::<BigInt, _>(page.fetch_limit())
        .bind::<BigInt, _>(page.sql_offset())
        .bind::<Nullable<Text>, _>(filter.thread.as_ref().map(|t| t.as_str()))
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        rows.into_iter().map(|r| parse_doc(r.doc)).collect()
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
