//! A session's work item, queued for the workers of its environment.

use std::sync::Arc;

use async_trait::async_trait;
use sessions::SessionWork;
use sessions::types::{Session, SessionId};
use shared::{ApiError, AuthContext, WorkspaceId};
use work::WorkService;

/// [`SessionWork`] over the work service.
#[derive(Debug, Clone)]
pub struct WorkQueue {
    /// The work items.
    pub work: Arc<WorkService>,
}

#[async_trait]
impl SessionWork for WorkQueue {
    async fn queue(
        &self,
        ctx: &AuthContext,
        session: &Session,
        environment: &serde_json::Value,
    ) -> Result<(), ApiError> {
        let self_hosted = environment.get("type").and_then(serde_json::Value::as_str) == Some("self_hosted");
        self.work.queue_for_loop(ctx, &session.environment_id, session.id.as_str(), self_hosted).await
    }

    async fn end(&self, ws: &WorkspaceId, session: &SessionId) {
        if let Err(e) = self.work.end_session(ws, session.as_str()).await {
            tracing::warn!(session = %session, error = %e.message, "work: could not stop the session's work item");
        }
    }
}
