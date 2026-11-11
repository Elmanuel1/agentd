//! The rubric files a session's outcomes name, read for the grader and for
//! the agent's own view of its outcome.

use std::collections::HashMap;

use files::FilesService;
use sessions::events::{Event, EventPayload, Rubric, UserEvent};
use shared::{BackendError, WorkspaceId};

/// The text of each rubric file the outcomes in `log` name, by file id; a
/// file deleted since reads as an empty rubric.
pub(super) async fn read(
    files: &FilesService,
    ws: &WorkspaceId,
    log: &[Event],
) -> Result<HashMap<String, String>, BackendError> {
    let mut texts = HashMap::new();
    for e in log {
        let EventPayload::User(UserEvent::DefineOutcome { rubric: Rubric::File { file_id }, .. }) =
            &e.payload
        else {
            continue;
        };
        match files.text(ws, file_id).await {
            Ok(text) => drop(texts.insert(file_id.clone(), text)),
            Err(e) if e.status == 404 => {}
            Err(e) => return Err(BackendError::new(e)),
        }
    }
    Ok(texts)
}
