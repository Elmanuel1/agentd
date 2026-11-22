//! The session event log's reads.

use ::sessions::events::Event;
use ::sessions::store::{EventFilter, Head, NativeSessionEventReads, Order, Pos, StoreError, Tail};
use ::sessions::types::SessionId;
use diesel::sql_types::{Array, BigInt, Nullable, Text, Timestamptz};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::{Pagination, WorkspaceId};

use super::PgSessionEventLog;
use super::rows::{HeadRow, LastRow, TailRow};
use crate::schema::{session_events, sessions};
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
        Ok(self.engine_events(ws, id).await?.into_iter().map(|(e, _)| e).collect())
    }

    async fn engine_events(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
    ) -> Result<Vec<(Event, Option<serde_json::Value>)>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<(i64, serde_json::Value)> = session_events::table
            .filter(session_events::workspace_id.eq(ws.as_str()))
            .filter(session_events::session_id.eq(id.as_str()))
            .order(session_events::seq.asc())
            .select((session_events::seq, session_events::doc))
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        // Each event's kept object names its `seq`, which a checkpoint records.
        rows.into_iter()
            .map(|(seq, doc)| {
                let mut kept = doc.get("engine").cloned().unwrap_or_else(|| serde_json::json!({}));
                if let Some(k) = kept.as_object_mut() {
                    k.insert("seq".into(), serde_json::json!(seq));
                }
                Ok((parse_doc(doc)?, Some(kept)))
            })
            .collect()
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
        // The session row comes back with no event when nothing matched, and
        // no row at all once the session is gone.
        let rows: Vec<TailRow> = diesel::sql_query(format!(
            "SELECT s.doc->>'archived_at' IS NOT NULL AS archived, e.xid::text::bigint AS xid, e.seq, e.doc
             FROM sessions s
             LEFT JOIN session_events e ON e.workspace_id = s.workspace_id AND e.session_id = s.session_id
               AND (((e.xid::text::bigint, e.seq) > ($3, $4) AND e.{SETTLED}) OR e.seq = ANY($5))
             WHERE s.workspace_id = $1 AND s.session_id = $2
             ORDER BY e.xid, e.seq LIMIT $6"
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
        let Some(archived) = <[TailRow]>::first(&rows).map(|r| r.archived) else {
            return Ok(None);
        };
        let events = rows
            .into_iter()
            .filter_map(|r| r.xid.zip(r.seq).zip(r.doc))
            .map(|((xid, seq), doc)| parse_doc(doc).map(|e| (Pos { xid, seq }, e)))
            .collect::<Result<_, StoreError>>()?;
        Ok(Some(Tail { archived, events }))
    }

    async fn head(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Head, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let last: Option<LastRow> = diesel::sql_query(format!(
            "SELECT xid::text::bigint AS xid, seq FROM session_events
             WHERE workspace_id = $1 AND session_id = $2 AND {SETTLED} ORDER BY xid DESC, seq DESC LIMIT 1"
        ))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .get_result(&mut conn)
        .await
        .optional()
        .map_err(diesel_be)?;
        let head: HeadRow = diesel::sql_query(format!(
            "SELECT coalesce(array_agg(seq ORDER BY seq) FILTER (WHERE {QUEUED}), '{{}}') AS queued
             FROM session_events WHERE workspace_id = $1 AND session_id = $2"
        ))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .get_result(&mut conn)
        .await
        .map_err(diesel_be)?;
        let pos = last.map(|l| Pos { xid: l.xid, seq: l.seq }).unwrap_or_default();
        Ok(Head { pos, queued: head.queued })
    }
}
