//! What a held step's write lands: its events, the stamps of the client
//! events it took, and its change to the session's row.

use ::sessions::steps::{Claim, StepWrite};
use ::sessions::store::StoreError;
use ::sessions::types::Session;
use diesel::OptionalExtension;
use diesel::sql_types::{Jsonb, Text};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::Timestamp;

use super::super::session_events::{insert, stamp};
use super::KEY;
use super::rows::Doc;
use crate::entities::webhook_deliveries::status_changed;
use crate::{enum_str, to_doc};

/// The step's events and session changes, after the held step was checked.
pub(super) async fn land(
    c: &mut AsyncPgConnection,
    claim: &Claim,
    write: &StepWrite<'_>,
) -> Result<(), diesel::result::Error> {
    let (ws, id) = (&claim.workspace, &claim.session);
    stamp(c, ws, id, write.read, write.at).await?;
    let ids: Vec<&str> = write.events.iter().map(|e| e.id.as_str()).collect();
    let mut docs =
        write.events.iter().map(to_doc).collect::<Result<Vec<_>, StoreError>>().map_err(serde_be)?;
    for (doc, event) in docs.iter_mut().zip(write.events) {
        let kept = write.engine.iter().find(|(e, _)| *e == event.id).map(|(_, kept)| kept.clone());
        let (Some(kept), Some(doc)) = (kept, doc.as_object_mut()) else { continue };
        // A checkpoint's covered seq at the path V11's unique index reads.
        let seq = kept.get("checkpoint").and_then(|c| c.get("through_seq")).cloned();
        doc.extend(seq.map(|seq| ("checkpoint".to_owned(), serde_json::json!({ "through_seq": seq }))));
        doc.insert("engine".into(), kept);
    }
    if insert(c, ws, id, &ids, docs).await? != ids.len() {
        return Err(diesel::result::Error::RollbackTransaction);
    }
    // A write that leaves the row as it is takes no lock on it; a step of a
    // session that is gone, with nothing to write, just ends.
    let Some(mut current) = row(c, claim, "").await? else {
        return match write.events.is_empty() {
            true => Ok(()),
            false => Err(diesel::result::Error::RollbackTransaction),
        };
    };
    if !(write.session)(&mut current) {
        return Ok(());
    }
    let mut session = row(c, claim, "FOR UPDATE").await?.ok_or(diesel::result::Error::RollbackTransaction)?;
    let before = session.status;
    if !(write.session)(&mut session) {
        return Ok(());
    }
    session.updated_at = Timestamp::now();
    let doc =
        serde_json::to_value(&session).map_err(|e| diesel::result::Error::SerializationError(e.into()))?;
    diesel::sql_query(format!("UPDATE sessions SET status = $3, doc = $4 WHERE {KEY}"))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<Text, _>(enum_str(&session.status))
        .bind::<Jsonb, _>(doc)
        .execute(c)
        .await?;
    match (session.status == before, write.deliveries) {
        (false, Some(deliveries)) => status_changed(c, ws, id, |org| deliveries(org, session.status)).await,
        _ => Ok(()),
    }
}

/// The session's row, read `lock`ed as given; `None` once it is gone.
async fn row(
    c: &mut AsyncPgConnection,
    claim: &Claim,
    lock: &str,
) -> Result<Option<Session>, diesel::result::Error> {
    let row: Option<Doc> = diesel::sql_query(format!("SELECT doc FROM sessions WHERE {KEY} {lock}"))
        .bind::<Text, _>(claim.workspace.as_str())
        .bind::<Text, _>(claim.session.as_str())
        .get_result(c)
        .await
        .optional()?;
    let Some(row) = row else { return Ok(None) };
    serde_json::from_value(row.doc)
        .map(Some)
        .map_err(|e| diesel::result::Error::DeserializationError(e.into()))
}

fn serde_be(e: StoreError) -> diesel::result::Error {
    diesel::result::Error::SerializationError(Box::new(e))
}
