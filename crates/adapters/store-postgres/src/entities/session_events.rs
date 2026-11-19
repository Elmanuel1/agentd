//! `SessionEventLog` on Postgres via Diesel: the shared append-only event log
//! (SSE tails `session_events` by `seq`).

mod reads;
mod writes;

pub(crate) use writes::{insert, lock, stamp};

use ::sessions::events::{Event, EventId};
use ::sessions::store::{AnswerCheck, NativeSessionEventLog, StoreError, Wake};
use ::sessions::types::SessionId;
use diesel::sql_types::{Array, Text};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use shared::{ApiError, Timestamp, WorkspaceId};

use super::loop_steps::enqueue;
use crate::schema::session_events;
use crate::{DocRow, PgBackend, diesel_be, parse_doc, pool_be, to_doc};

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

    async fn append(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        events: &[Event],
        wake: Wake,
        log: Log,
    ) -> Result<bool, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
        let docs = events.iter().map(to_doc).collect::<Result<Vec<_>, StoreError>>()?;
        conn.transaction::<_, diesel::result::Error, _>(async move |c| {
            lock(c, id).await?;
            if log == Log::Empty && logged(c, ws, id).await? {
                return Ok(false);
            }
            if insert(c, ws, id, &ids, docs).await? != ids.len() {
                return Err(diesel::result::Error::NotFound);
            }
            enqueue(c, ws, id, wake).await.map(|()| true)
        })
        .await
        .map_err(log_be)
    }
}

#[derive(PartialEq, Eq)]
enum Log {
    Any,
    Empty,
}

impl NativeSessionEventLog for PgSessionEventLog {
    async fn append_events(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        events: &[Event],
        wake: Wake,
    ) -> Result<(), StoreError> {
        self.append(ws, id, events, wake, Log::Any).await.map(drop)
    }

    async fn append_if_empty(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        events: &[Event],
        wake: Wake,
    ) -> Result<bool, StoreError> {
        self.append(ws, id, events, wake, Log::Empty).await
    }

    async fn update_processed_at(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        ids: &[EventId],
        at: Timestamp,
    ) -> Result<Vec<Event>, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        stamp(&mut conn, ws, id, ids, at).await.map_err(diesel_be)?.into_iter().map(parse_doc).collect()
    }

    async fn append_answers(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        calls: &[EventId],
        events: &[Event],
        check: &AnswerCheck<'_>,
        wake: Wake,
    ) -> Result<Result<(), ApiError>, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let calls: Vec<&str> = calls.iter().map(EventId::as_str).collect();
        let ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
        conn.transaction::<_, diesel::result::Error, _>(async move |c| {
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
            let found =
                rows.into_iter().map(|r| serde_json::from_value(r.doc)).collect::<Result<Vec<Event>, _>>();
            let threads =
                match check(&found.map_err(|e| diesel::result::Error::DeserializationError(e.into()))?) {
                    Ok(threads) => threads,
                    Err(refused) => return Ok(Err(refused)),
                };
            let answers = events
                .iter()
                .zip(threads)
                .map(|(e, session_thread_id)| Event { session_thread_id, ..e.clone() });
            let docs = answers
                .map(|e| to_doc(&e))
                .collect::<Result<Vec<_>, StoreError>>()
                .map_err(|e| diesel::result::Error::SerializationError(e.to_string().into()))?;
            if insert(c, ws, id, &ids, docs).await? != events.len() {
                return Err(diesel::result::Error::NotFound);
            }
            enqueue(c, ws, id, wake).await.map(Ok)
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

async fn logged(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
) -> Result<bool, diesel::result::Error> {
    let seq: Option<i64> = session_events::table
        .filter(session_events::workspace_id.eq(ws.as_str()))
        .filter(session_events::session_id.eq(id.as_str()))
        .select(session_events::seq)
        .first(c)
        .await
        .optional()?;
    Ok(seq.is_some())
}

fn log_be(e: diesel::result::Error) -> StoreError {
    match e {
        diesel::result::Error::NotFound => StoreError::NotFound,
        e => diesel_be(e),
    }
}
