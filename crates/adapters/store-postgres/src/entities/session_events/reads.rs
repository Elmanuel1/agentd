//! The session event log's reads.

use ::sessions::events::Event;
use ::sessions::store::{EventFilter, Order, SessionEventReads, StoreError, Tail};
use ::sessions::types::SessionId;
use async_trait::async_trait;
use diesel::sql_types::{Array, BigInt, Bool, Jsonb, Nullable, Text, Timestamptz};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::{Pagination, WorkspaceId};

use super::PgSessionEventLog;
use crate::schema::{session_events, sessions};
use crate::{diesel_be, parse_doc, pool_be, DocRow};

impl PgSessionEventLog {
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
impl SessionEventReads for PgSessionEventLog {
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
        let order = match filter.order {
            Order::Desc => "DESC",
            Order::Asc => "ASC",
        };
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

    async fn tail(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        after: i64,
        pending: &[i64],
        limit: usize,
    ) -> Result<Option<Tail>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        // The session row comes back with no event when nothing matched, and
        // no row at all once the session is gone.
        let rows: Vec<TailRow> = diesel::sql_query(
            "SELECT s.doc->>'archived_at' IS NOT NULL AS archived, e.seq, e.doc FROM sessions s
             LEFT JOIN session_events e ON e.workspace_id = s.workspace_id AND e.session_id = s.session_id
               AND (e.seq > $3 OR e.seq = ANY($4))
             WHERE s.workspace_id = $1 AND s.session_id = $2
             ORDER BY e.seq LIMIT $5",
        )
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<BigInt, _>(after)
        .bind::<Array<BigInt>, _>(pending)
        .bind::<BigInt, _>(i64::try_from(limit + pending.len()).unwrap_or(i64::MAX))
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        let Some(archived) = <[TailRow]>::first(&rows).map(|r| r.archived) else {
            return Ok(None);
        };
        let events = rows
            .into_iter()
            .filter_map(|r| r.seq.zip(r.doc))
            .map(|(seq, doc)| parse_doc(doc).map(|e| (seq, e)))
            .collect::<Result<_, StoreError>>()?;
        Ok(Some(Tail { archived, events }))
    }

    async fn head(&self, ws: &WorkspaceId, id: &SessionId) -> Result<i64, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let head: Option<i64> = session_events::table
            .filter(session_events::workspace_id.eq(ws.as_str()))
            .filter(session_events::session_id.eq(id.as_str()))
            .select(diesel::dsl::max(session_events::seq))
            .first(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(head.unwrap_or(0))
    }
}

#[derive(diesel::QueryableByName)]
struct TailRow {
    #[diesel(sql_type = Bool)]
    archived: bool,
    #[diesel(sql_type = Nullable<BigInt>)]
    seq: Option<i64>,
    #[diesel(sql_type = Nullable<Jsonb>)]
    doc: Option<serde_json::Value>,
}
