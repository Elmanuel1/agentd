//! The loop's reads of the event log, each event with the `engine` object
//! stored beside it.

use ::sessions::store::{EngineEvents, NativeEngineReads, StoreError};
use ::sessions::types::{SessionId, ThreadId};
use diesel::sql_types::Text;
use diesel_async::RunQueryDsl;
use shared::WorkspaceId;

use super::PgSessionEventLog;
use super::rows::{SeqDoc, Sum};
use crate::{diesel_be, parse_doc, pool_be};

/// One thread's events, with what its step reads of the rest: the client
/// events answering its calls or none, the session's events and the
/// outcomes' evaluation spans. The primary thread is keyed as ''.
const THREAD: &str = "SELECT seq, doc FROM session_events e
     WHERE e.workspace_id = $1 AND e.session_id = $2 AND (
       coalesce(e.doc->>'session_thread_id', '') = $3
       OR e.doc->>'type' LIKE 'session.%'
       OR e.doc->>'type' LIKE 'span.outcome_evaluation%'
       OR e.doc->>'type' = 'system.message'
       OR (e.doc->>'type' LIKE 'user.%' AND (
         coalesce(e.doc->>'tool_use_id', e.doc->>'custom_tool_use_id') IS NULL
         OR coalesce(e.doc->>'tool_use_id', e.doc->>'custom_tool_use_id') IN (
           SELECT m.event_id FROM session_events m
           WHERE m.workspace_id = $1 AND m.session_id = $2
             AND coalesce(m.doc->>'session_thread_id', '') = $3))))
     ORDER BY e.seq";

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

    async fn thread_events(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        thread: Option<&ThreadId>,
    ) -> Result<EngineEvents, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<SeqDoc> = diesel::sql_query(THREAD)
            .bind::<Text, _>(ws.as_str())
            .bind::<Text, _>(id.as_str())
            .bind::<Text, _>(thread.map_or("", ThreadId::as_str))
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        engine(rows)
    }

    async fn engine_sum(&self, ws: &WorkspaceId, id: &SessionId, key: &str) -> Result<u64, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let sum: Sum = diesel::sql_query(
            "SELECT coalesce(sum((doc->'engine'->>$3)::bigint), 0)::bigint AS total FROM session_events
             WHERE workspace_id = $1 AND session_id = $2 AND doc->'engine' ? $3",
        )
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<Text, _>(key)
        .get_result(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(u64::try_from(sum.total).unwrap_or_default())
    }
}
