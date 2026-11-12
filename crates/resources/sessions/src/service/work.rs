//! A session's work item: queued when the session is created, stopped when it
//! is archived.

use shared::{ApiError, AuthContext, WorkspaceId};

use super::SessionsService;
use crate::types::{Session, SessionId};

impl SessionsService {
    /// Put `session`'s work item on its queue. Called before the session row
    /// is written, so work that cannot be queued leaves no session behind.
    pub(super) async fn queue_work(
        &self,
        ctx: &AuthContext,
        session: &Session,
        environment: &serde_json::Value,
    ) -> Result<(), ApiError> {
        match &self.work {
            Some(work) => work.queue(ctx, session, environment).await,
            None => Ok(()),
        }
    }

    /// Stop the session's work item.
    pub(super) async fn end_work(&self, ws: &WorkspaceId, id: &SessionId) {
        if let Some(work) = &self.work {
            work.end(ws, id).await;
        }
    }
}
