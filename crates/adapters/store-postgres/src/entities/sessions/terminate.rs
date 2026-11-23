//! A session's archive, in the archive's own commit: its work item stops,
//! and when the archive terminates it, as live's does (U3), the log gains
//! `session.status_terminated`.

use ::sessions::events::{Event, EventPayload, SessionEvent};
use ::sessions::types::{Session, SessionStatus};
use ::work::store::Reach;
use diesel_async::AsyncPgConnection;
use shared::{Timestamp, WorkspaceId};

use super::super::session_events::insert;
use super::super::work_items::stop::stop;

/// What the archive of `read` into `s` writes beside the row, under the
/// session's lock the caller took first.
pub(super) async fn archived(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    read: &Session,
    s: &Session,
) -> Result<(), diesel::result::Error> {
    stop(c, ws, s.id.as_str(), &Reach::default(), false, Timestamp::now()).await?;
    if read.status == s.status || s.status != SessionStatus::Terminated {
        return Ok(());
    }
    let mut event = Event::new(EventPayload::Session(SessionEvent::StatusTerminated {}));
    event.processed_at = Some(Timestamp::now());
    let doc =
        serde_json::to_value(&event).map_err(|e| diesel::result::Error::SerializationError(Box::new(e)))?;
    insert(c, ws, &s.id, &[event.id.as_str()], vec![doc]).await?;
    Ok(())
}
