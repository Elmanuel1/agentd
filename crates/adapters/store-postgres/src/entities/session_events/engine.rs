//! The loop's reads of the event log, each event with the `engine` object
//! stored beside it.

use ::sessions::store::{Calls, EngineEvents, NativeEngineReads, StoreError};
use ::sessions::types::{SessionId, ThreadId};
use diesel::sql_types::{Nullable, Text};
use diesel_async::RunQueryDsl;
use shared::WorkspaceId;

use super::PgSessionEventLog;
use super::rows::{CallsRow, SeqDoc};
use crate::{diesel_be, parse_doc, pool_be};

/// Each thread's events from its latest checkpoint's covered event (the
/// snapshot it resumes from), only thread `$3`'s when it is set, with every
/// session event, the outcomes' spans and the client events that answer no
/// call. The cuts read V11's index; the rest V27's.
const STEP: &str = "WITH cuts AS (
       SELECT coalesce(doc->>'session_thread_id', '') AS thread_id,
              max((doc->'checkpoint'->>'through_seq')::bigint) AS seq
       FROM session_events
       WHERE workspace_id = $1 AND session_id = $2 AND doc->>'type' = 'agent.thread_context_compacted'
       GROUP BY 1)
     SELECT e.seq, e.doc FROM session_events e LEFT JOIN cuts c ON c.thread_id = e.thread_id
     WHERE e.workspace_id = $1 AND e.session_id = $2 AND (
       (($3::text IS NULL OR e.thread_id = $3) AND e.seq >= coalesce(c.seq, 0))
       OR e.event_type LIKE 'session.%'
       OR e.event_type LIKE 'span.outcome_evaluation%'
       OR e.event_type IN ('user.message', 'user.interrupt', 'user.tool_confirmation', 'user.define_outcome',
                           'system.message'))
     ORDER BY e.seq";

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

impl PgSessionEventLog {
    async fn step(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        thread: Option<&str>,
    ) -> Result<EngineEvents, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<SeqDoc> = diesel::sql_query(STEP)
            .bind::<Text, _>(ws.as_str())
            .bind::<Text, _>(id.as_str())
            .bind::<Nullable<Text>, _>(thread)
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
        self.step(ws, id, None).await
    }

    async fn thread_events(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        thread: Option<&ThreadId>,
    ) -> Result<EngineEvents, StoreError> {
        self.step(ws, id, Some(thread.map_or("", ThreadId::as_str))).await
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
