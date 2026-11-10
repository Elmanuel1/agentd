//! `SessionEventLog` on Postgres via Diesel: the shared append-only event log
//! (SSE tails `session_events` by `seq`).

mod reads;

use ::sessions::events::{Event, EventId};
use ::sessions::store::{AnswerCheck, SessionEventLog, StoreError};
use ::sessions::types::SessionId;
use async_trait::async_trait;
use diesel::sql_types::{Array, Jsonb, Text};
use diesel::{ExpressionMethods, QueryDsl};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use shared::{ApiError, WorkspaceId};

use crate::schema::session_events;
use crate::{diesel_be, pool_be, to_doc, DocRow, PgBackend};

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
    async fn append_event(&self, ws: &WorkspaceId, id: &SessionId, event: Event) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let inserted = insert(&mut conn, ws, id, &[event.id.as_str()], vec![to_doc(&event)?])
            .await
            .map_err(diesel_be)?;
        if inserted == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn replace_event(&self, ws: &WorkspaceId, id: &SessionId, event: Event) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
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
                // F1's per-session append lock: a second writer answering the
                // same call waits here, then reads the first one's result.
                diesel::sql_query("SELECT pg_advisory_xact_lock(hashtext($1))")
                    .bind::<Text, _>(id.as_str())
                    .execute(c)
                    .await?;
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
        .map_err(|e| match e {
            diesel::result::Error::NotFound => StoreError::NotFound,
            e => diesel_be(e),
        })
    }

    async fn delete_events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
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
