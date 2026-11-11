//! `SessionEventLog` on Postgres via Diesel: the shared append-only event log
//! (SSE tails `session_events` by `seq`).

mod reads;

use ::sessions::events::{Event, EventId};
use ::sessions::store::{AnswerCheck, SessionEventLog, StoreError};
use ::sessions::types::SessionId;
use async_trait::async_trait;
use diesel::sql_types::{Array, BigInt, Jsonb, Text};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use shared::{ApiError, Timestamp, WorkspaceId};

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
}

#[async_trait]
impl SessionEventLog for PgSessionEventLog {
    async fn append_events(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        events: &[Event],
    ) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
        let docs = events.iter().map(to_doc).collect::<Result<Vec<_>, StoreError>>()?;
        conn.transaction::<_, diesel::result::Error, _>(|c| {
            async move {
                lock(c, id).await?;
                if insert(c, ws, id, &ids, docs).await? != ids.len() {
                    return Err(diesel::result::Error::NotFound);
                }
                Ok(())
            }
            .scope_boxed()
        })
        .await
        .map_err(log_be)
    }

    async fn update_processed_at(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        ids: &[EventId],
        at: Timestamp,
    ) -> Result<Vec<Event>, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let ids: Vec<&str> = ids.iter().map(EventId::as_str).collect();
        let mut rows: Vec<SeqDocRow> = diesel::sql_query(
            "UPDATE session_events SET doc = jsonb_set(doc, '{processed_at}', to_jsonb($4::text))
             WHERE workspace_id = $1 AND session_id = $2 AND event_id = ANY($3) AND doc->>'processed_at' IS NULL
             RETURNING seq, doc",
        )
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<Array<Text>, _>(&ids)
        .bind::<Text, _>(at.to_wire())
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        rows.sort_by_key(|r| r.seq);
        rows.into_iter().map(|r| parse_doc(r.doc)).collect()
    }

    async fn append_answers(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        calls: &[EventId],
        events: &[Event],
        check: &AnswerCheck<'_>,
    ) -> Result<Result<(), ApiError>, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let calls: Vec<&str> = calls.iter().map(EventId::as_str).collect();
        let ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
        let docs = events.iter().map(to_doc).collect::<Result<Vec<_>, StoreError>>()?;
        conn.transaction::<_, diesel::result::Error, _>(|c| {
            async move {
                // A second writer answering the same call waits here, then
                // reads the first one's result.
                lock(c, id).await?;
                let rows: Vec<DocRow> = diesel::sql_query(
                    "SELECT doc FROM session_events WHERE workspace_id = $1 AND session_id = $2 AND (event_id = \
                     ANY($3) OR doc->>'tool_use_id' = ANY($3) OR doc->>'custom_tool_use_id' = ANY($3))",
                )
                .bind::<Text, _>(ws.as_str())
                .bind::<Text, _>(id.as_str())
                .bind::<Array<Text>, _>(&calls)
                .load(c)
                .await?;
                let found = rows.into_iter().map(|r| serde_json::from_value(r.doc)).collect::<Result<Vec<Event>, _>>();
                if let Err(refused) = check(&found.map_err(|e| diesel::result::Error::DeserializationError(e.into()))?) {
                    return Ok(Err(refused));
                }
                if insert(c, ws, id, &ids, docs).await? != events.len() {
                    return Err(diesel::result::Error::NotFound);
                }
                Ok(Ok(()))
            }
            .scope_boxed()
        })
        .await
        .map_err(log_be)
    }

    async fn delete_events_of(&self, sessions: &[(WorkspaceId, SessionId)]) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let (ws, ids): (Vec<&str>, Vec<&str>) =
            sessions.iter().map(|(w, s)| (w.as_str(), s.as_str())).unzip();
        diesel::sql_query(
            "DELETE FROM session_events USING unnest($1::text[], $2::text[]) AS v(workspace_id, session_id)
             WHERE session_events.workspace_id = v.workspace_id AND session_events.session_id = v.session_id",
        )
        .bind::<Array<Text>, _>(ws)
        .bind::<Array<Text>, _>(ids)
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(())
    }
}

/// The session's append lock, held to the end of the transaction. Two
/// writers appending to one session at once would otherwise commit out of
/// `seq` order: T1 takes seq 10, T2 takes 11 and commits first, and a stream
/// reading after 9 moves its cursor to 11 and never sends 10.
pub(crate) async fn lock(c: &mut AsyncPgConnection, id: &SessionId) -> Result<(), diesel::result::Error> {
    diesel::sql_query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind::<Text, _>(id.as_str())
        .execute(c)
        .await?;
    Ok(())
}

fn log_be(e: diesel::result::Error) -> StoreError {
    match e {
        diesel::result::Error::NotFound => StoreError::NotFound,
        e => diesel_be(e),
    }
}

/// One event read with its log position.
#[derive(diesel::QueryableByName)]
pub(crate) struct SeqDocRow {
    #[diesel(sql_type = BigInt)]
    pub(crate) seq: i64,
    #[diesel(sql_type = Jsonb)]
    pub(crate) doc: serde_json::Value,
}

async fn insert(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
    events: &[&str],
    docs: Vec<serde_json::Value>,
) -> Result<usize, diesel::result::Error> {
    // One statement: the events land only while their session row exists, so
    // a delete that removed the session first leaves no orphan behind.
    diesel::sql_query(
        "INSERT INTO session_events (workspace_id, session_id, event_id, created_at, doc)
         SELECT s.workspace_id, s.session_id, e.event_id, now(), e.doc
         FROM sessions s, unnest($3::text[], $4::jsonb[]) WITH ORDINALITY AS e(event_id, doc, n)
         WHERE s.workspace_id = $1 AND s.session_id = $2 ORDER BY e.n",
    )
    .bind::<Text, _>(ws.as_str())
    .bind::<Text, _>(id.as_str())
    .bind::<Array<Text>, _>(events)
    .bind::<Array<Jsonb>, _>(docs)
    .execute(c)
    .await
}
