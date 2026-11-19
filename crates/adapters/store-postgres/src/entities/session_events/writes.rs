//! The session event log's writes that other writers share: the append lock, the insert and the stamp.

use ::sessions::events::EventId;
use ::sessions::types::SessionId;
use diesel::sql_types::{Array, BigInt, Jsonb, Text};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::{Timestamp, WorkspaceId};

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

/// Stamp `at` on those of `ids` not processed yet; their docs, in log order.
pub(crate) async fn stamp(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
    ids: &[EventId],
    at: Timestamp,
) -> Result<Vec<serde_json::Value>, diesel::result::Error> {
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
    .load(c)
    .await?;
    rows.sort_by_key(|r| r.seq);
    Ok(rows.into_iter().map(|r| r.doc).collect())
}

/// One event read with its log position.
#[derive(diesel::QueryableByName)]
struct SeqDocRow {
    #[diesel(sql_type = BigInt)]
    seq: i64,
    #[diesel(sql_type = Jsonb)]
    doc: serde_json::Value,
}

pub(crate) async fn insert(
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
