//! The loop's reads of the event log, each event with the `engine` object
//! stored beside it.

use ::sessions::store::{Calls, EngineEvents, NativeEngineReads, StoreError, ThreadRead};
use ::sessions::types::{SessionId, ThreadId};
use diesel::sql_types::{BigInt, Bool, Nullable, Text};
use diesel_async::RunQueryDsl;
use shared::WorkspaceId;

use super::PgSessionEventLog;
use super::rows::{CallsRow, SeqDoc};
use crate::{diesel_be, parse_doc, pool_be};

/// Each thread's events from its latest checkpoint's covered event (the
/// snapshot it resumes from), or from its first with `$4`, only thread `$3`'s
/// when it is set, with every session event, the outcomes' spans, the
/// client events that answer no call and the answers to its calls, which
/// need not carry its thread. The cuts read V11's index; the rest
/// V27's. A processed successful tool result comes with no content when no
/// prompt needs it: always with `$5` unset (the session's step), else once
/// the thread's last call since its checkpoint took `$5` tokens, for a
/// result before that call older than the newest `$6`, which the prompt
/// clears. Each event's document is read once: `picked` and `last` are
/// materialized, so no row is checked against every other.
const STEP: &str = "WITH cuts AS (
       SELECT coalesce(doc->>'session_thread_id', '') AS thread_id,
              max((doc->'checkpoint'->>'through_seq')::bigint) AS through, max(seq) AS at
       FROM session_events
       WHERE workspace_id = $1 AND session_id = $2 AND doc->>'type' = 'agent.thread_context_compacted'
       GROUP BY 1),
     picked AS MATERIALIZED (
       SELECT e.seq, e.event_id, e.doc, e.thread_id, e.event_type,
              e.event_type IN ('agent.tool_result', 'agent.mcp_tool_result', 'user.tool_result',
                               'user.custom_tool_result')
                AND e.doc->>'processed_at' IS NOT NULL
                AND NOT coalesce((e.doc->>'is_error')::boolean, false) AS clearable
       FROM session_events e
       LEFT JOIN cuts c ON c.thread_id = e.thread_id
       WHERE e.workspace_id = $1 AND e.session_id = $2 AND (
         (($3::text IS NULL OR e.thread_id = $3) AND ($4 OR e.seq >= coalesce(c.through, 0)))
         OR e.event_type LIKE 'session.%'
         OR e.event_type LIKE 'span.outcome_evaluation%'
         OR e.event_type IN ('user.message', 'user.interrupt', 'user.tool_confirmation',
                             'user.define_outcome', 'system.message')
         OR (e.event_type IN ('user.tool_result', 'user.custom_tool_result')
             AND coalesce(e.doc->>'tool_use_id', e.doc->>'custom_tool_use_id') IN (
               SELECT m.event_id FROM session_events m
               WHERE m.workspace_id = $1 AND m.session_id = $2 AND m.thread_id = $3)))),
     last AS MATERIALIZED (
       SELECT p.seq, coalesce((u->>'input_tokens')::bigint, 0) + coalesce((u->>'cache_read_input_tokens')::bigint, 0)
              + coalesce((u->>'cache_creation_input_tokens')::bigint, 0)
              + coalesce((u->>'output_tokens')::bigint, 0) AS tokens
       FROM picked p CROSS JOIN LATERAL (SELECT p.doc->'model_usage' AS u) x
       LEFT JOIN cuts c ON c.thread_id = p.thread_id
       LEFT JOIN picked st ON st.event_id = p.doc->>'model_request_start_id'
       WHERE p.thread_id = $3 AND p.event_type = 'span.model_request_end' AND p.seq > coalesce(c.at, 0)
         AND NOT coalesce((p.doc->>'is_error')::boolean, false)
         AND coalesce(st.doc->'engine'->>'compaction', '') <> 'true'
       ORDER BY p.seq DESC LIMIT 1),
     ranked AS (
       SELECT p.seq, p.doc, p.clearable,
              row_number() OVER (PARTITION BY p.clearable ORDER BY p.seq DESC) AS newest
       FROM picked p)
     SELECT r.seq, CASE
         WHEN r.clearable AND ($5::bigint IS NULL OR (r.newest > $6 AND l.tokens >= $5 AND r.seq < l.seq))
         THEN jsonb_set(r.doc, '{content}', '[]'::jsonb) ELSE r.doc END AS doc
     FROM ranked r LEFT JOIN last l ON true
     ORDER BY r.seq";

/// The session's model calls added up; a cost is kept at `$3`.
const CALLS: &str = "SELECT
       coalesce(sum((u->>'cache_read_input_tokens')::bigint) FILTER (WHERE ok), 0)::bigint AS cache_read,
       coalesce(sum((u->>'input_tokens')::bigint + coalesce((u->>'cache_creation_input_tokens')::bigint, 0))
         FILTER (WHERE ok), 0)::bigint AS input,
       coalesce(sum((u->>'output_tokens')::bigint) FILTER (WHERE ok), 0)::bigint AS output,
       count(*) FILTER (WHERE ok AND NOT coalesce(engine ? $3, false)) AS unpriced,
       coalesce(sum((engine->>$3)::bigint), 0)::bigint AS cost
     FROM (SELECT doc->'model_usage' AS u, doc->'engine' AS engine,
                  NOT coalesce((doc->>'is_error')::boolean, false) AS ok
           FROM session_events
           WHERE workspace_id = $1 AND session_id = $2 AND event_type = 'span.model_request_end') calls";

/// Each event with its kept object, which names the event's `seq`, as a
/// checkpoint records it.
fn engine(rows: Vec<SeqDoc>) -> Result<EngineEvents, StoreError> {
    rows.into_iter()
        .map(|SeqDoc { seq, doc }| {
            let mut kept = doc.get("engine").cloned().unwrap_or_else(|| serde_json::json!({}));
            if let Some(k) = kept.as_object_mut() {
                k.insert("seq".into(), serde_json::json!(seq));
            }
            Ok((parse_doc(doc)?, Some(kept)))
        })
        .collect()
}

/// A step's read: of every thread (`None`) or one, as `how` says.
struct Step<'a> {
    thread: Option<&'a str>,
    how: Option<ThreadRead>,
}

impl PgSessionEventLog {
    async fn step(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        read: Step<'_>,
    ) -> Result<EngineEvents, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let clear_at = read.how.map(|h| i64::try_from(h.clear_at).unwrap_or(i64::MAX));
        let keep = read.how.map_or(0, |h| i64::try_from(h.keep).unwrap_or(i64::MAX));
        let rows: Vec<SeqDoc> = diesel::sql_query(STEP)
            .bind::<Text, _>(ws.as_str())
            .bind::<Text, _>(id.as_str())
            .bind::<Nullable<Text>, _>(read.thread)
            .bind::<Bool, _>(read.how.is_some_and(|h| h.whole))
            .bind::<Nullable<BigInt>, _>(clear_at)
            .bind::<BigInt, _>(keep)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        engine(rows)
    }
}

impl NativeEngineReads for PgSessionEventLog {
    async fn engine_events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<EngineEvents, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<SeqDoc> = diesel::sql_query(
            "SELECT seq, doc FROM session_events WHERE workspace_id = $1 AND session_id = $2 ORDER BY seq",
        )
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        engine(rows)
    }

    async fn step_events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<EngineEvents, StoreError> {
        self.step(ws, id, Step { thread: None, how: None }).await
    }

    async fn thread_events(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        thread: Option<&ThreadId>,
        how: ThreadRead,
    ) -> Result<EngineEvents, StoreError> {
        self.step(ws, id, Step { thread: Some(thread.map_or("", ThreadId::as_str)), how: Some(how) }).await
    }

    async fn calls(&self, ws: &WorkspaceId, id: &SessionId, cost: &str) -> Result<Calls, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let row: CallsRow = diesel::sql_query(CALLS)
            .bind::<Text, _>(ws.as_str())
            .bind::<Text, _>(id.as_str())
            .bind::<Text, _>(cost)
            .get_result(&mut conn)
            .await
            .map_err(diesel_be)?;
        let n = |v: i64| u64::try_from(v).unwrap_or_default();
        Ok(Calls {
            cache_read_input_tokens: n(row.cache_read),
            input_tokens: n(row.input),
            output_tokens: n(row.output),
            unpriced: n(row.unpriced),
            cost: n(row.cost),
        })
    }
}
