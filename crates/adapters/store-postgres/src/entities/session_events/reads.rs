//! The session event log's reads.

use ::sessions::events::Event;
use ::sessions::store::{
    EventFilter, Head, NativeEngineReads, NativeSessionEventReads, Order, Pos, StoreError, Tail,
};
use ::sessions::types::SessionId;
use diesel::sql_types::{Array, BigInt, Nullable, Text, Timestamptz};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::{Pagination, WorkspaceId};

use super::PgSessionEventLog;
use super::rows::{CountRow, HeadRow, StateRow, TailRow};
use crate::schema::sessions;
use crate::{DocRow, diesel_be, parse_doc, pool_be};

/// Committed by every transaction older than the oldest one still running:
/// an event below it can no longer be overtaken by one that commits late.
const SETTLED: &str = "xid < pg_snapshot_xmin(pg_current_snapshot())";

/// A client event the agent has not taken yet.
const QUEUED: &str =
    "doc->>'processed_at' IS NULL AND (doc->>'type' LIKE 'user.%' OR doc->>'type' = 'system.message')";

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

impl NativeSessionEventReads for PgSessionEventLog {
    async fn events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Vec<Event>, StoreError> {
        self.require_session(ws, id).await?;
        Ok(NativeEngineReads::engine_events(self, ws, id).await?.into_iter().map(|(e, _)| e).collect())
    }

    async fn unprocessed_events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Vec<Event>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<DocRow> = diesel::sql_query(format!(
            "SELECT doc FROM session_events WHERE workspace_id = $1 AND session_id = $2 AND {QUEUED} ORDER BY seq"
        ))
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
               AND {SETTLED}
               AND (cardinality($3::text[]) = 0 OR doc->>'type' = ANY($3))
               AND ($4::timestamptz IS NULL OR {processed} > $4)
               AND ($5::timestamptz IS NULL OR {processed} >= $5)
               AND ($6::timestamptz IS NULL OR {processed} < $6)
               AND ($7::timestamptz IS NULL OR {processed} <= $7)
               AND (CASE WHEN $10::text IS NULL THEN
                      doc->>'session_thread_id' IS NULL OR doc->>'type' LIKE 'session.%'
                      OR (doc->>'type' = 'agent.tool_use' AND jsonb_typeof(doc->'evaluation') = 'object')
                      OR doc->>'type' = 'agent.custom_tool_use'
                    ELSE doc->>'session_thread_id' = $10 AND doc->>'type' <> 'session.thread_created' END)
             ORDER BY {processed} {order} NULLS LAST, xid {order}, seq {order}
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
        after: Pos,
        pending: &[i64],
        limit: usize,
    ) -> Result<Option<Tail>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        // An archived session reads as archived only once no event is left
        // unsettled, counted before the events are read: an event settled
        // then is settled for the read after it. No row once the session is gone.
        let state: Option<StateRow> =
            diesel::sql_query("SELECT doc->>'archived_at' IS NOT NULL AS archived FROM sessions WHERE workspace_id = $1 AND session_id = $2")
                .bind::<Text, _>(ws.as_str())
                .bind::<Text, _>(id.as_str())
                .get_result(&mut conn)
                .await
                .optional()
                .map_err(diesel_be)?;
        let Some(state) = state else { return Ok(None) };
        let unsettled = match state.archived {
            false => 0,
            true => {
                let row: CountRow = diesel::sql_query(format!(
                    "SELECT count(*) AS n FROM session_events
                     WHERE workspace_id = $1 AND session_id = $2 AND NOT {SETTLED}"
                ))
                .bind::<Text, _>(ws.as_str())
                .bind::<Text, _>(id.as_str())
                .get_result(&mut conn)
                .await
                .map_err(diesel_be)?;
                row.n
            }
        };
        // Only settled events are served, a pending one too, so the cursor
        // never passes an earlier one still being written.
        let rows: Vec<TailRow> = diesel::sql_query(format!(
            "SELECT xid::text::bigint AS xid, seq, doc FROM session_events
             WHERE workspace_id = $1 AND session_id = $2
               AND {SETTLED} AND ((xid::text::bigint, seq) > ($3, $4) OR seq = ANY($5))
             ORDER BY xid, seq LIMIT $6"
        ))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<BigInt, _>(after.xid)
        .bind::<BigInt, _>(after.seq)
        .bind::<Array<BigInt>, _>(pending)
        .bind::<BigInt, _>(i64::try_from(limit + pending.len()).unwrap_or(i64::MAX))
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        let archived = state.archived && unsettled == 0;
        let events = rows
            .into_iter()
            .map(|r| parse_doc(r.doc).map(|e| (Pos { xid: r.xid, seq: r.seq }, e)))
            .collect::<Result<_, StoreError>>()?;
        Ok(Some(Tail { archived, events }))
    }

    async fn head(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Head, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let head: HeadRow = diesel::sql_query(format!(
            "SELECT (array_agg(xid::text::bigint ORDER BY xid DESC, seq DESC) FILTER (WHERE {SETTLED}))[1] AS xid,
                    (array_agg(seq ORDER BY xid DESC, seq DESC) FILTER (WHERE {SETTLED}))[1] AS seq,
                    coalesce(array_agg(seq ORDER BY seq) FILTER (WHERE {QUEUED}), '{{}}') AS queued
             FROM session_events WHERE workspace_id = $1 AND session_id = $2"
        ))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .get_result(&mut conn)
        .await
        .map_err(diesel_be)?;
        let pos = head.xid.zip(head.seq).map(|(xid, seq)| Pos { xid, seq }).unwrap_or_default();
        Ok(Head { pos, queued: head.queued })
    }
}
