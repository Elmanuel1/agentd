//! A session's archive, in the archive's own commit: its work item stops,
//! and when the archive terminates it, as live's does (U3), the log gains
//! `session.status_terminated` and the archive's webhook deliveries.

use ::sessions::events::{Event, EventPayload, SessionEvent};
use ::sessions::types::{Session, SessionStatus};
use ::work::store::Reach;
use diesel_async::AsyncPgConnection;
use shared::{Timestamp, WorkspaceId};
use webhooks::Delivery;

use super::super::session_events::{insert, lock};
use super::super::webhook_deliveries::insert_deliveries;
use super::super::work_items::stop::stop;

/// What the archive of `read` into `s` writes beside the row.
pub(super) async fn archived(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    read: &Session,
    s: &Session,
    sent: &[Delivery],
) -> Result<(), diesel::result::Error> {
    stop(c, ws, s.id.as_str(), &Reach::default(), false, Timestamp::now()).await?;
    if read.status == s.status || s.status != SessionStatus::Terminated {
        return Ok(());
    }
    lock(c, &s.id).await?;
    let mut event = Event::new(EventPayload::Session(SessionEvent::StatusTerminated {}));
    event.processed_at = Some(Timestamp::now());
    let doc =
        serde_json::to_value(&event).map_err(|e| diesel::result::Error::SerializationError(Box::new(e)))?;
    insert(c, ws, &s.id, &[event.id.as_str()], vec![doc]).await?;
    insert_deliveries(c, sent).await
}
