use shared::ports::{JobKind, JobSpec, RuntimeError};
use shared::{ApiError, AuthContext};

use super::SessionsService;
use crate::types::{Session, SessionId};

impl SessionsService {
    /// Start `session`'s sandbox from its resolved environment. Called before
    /// the session row is written, so a sandbox that cannot start leaves no
    /// session behind.
    pub(super) async fn launch_sandbox(
        &self,
        ctx: &AuthContext,
        session: &Session,
        environment: serde_json::Value,
    ) -> Result<(), ApiError> {
        let Some(runtime) = &self.sandbox else {
            return Ok(());
        };
        let job = JobSpec {
            kind: JobKind::Session,
            target_id: session.id.to_string(),
            workspace_id: ctx.workspace.as_str().to_owned(),
            environment_id: session.environment_id.clone(),
            environment,
            env: Default::default(),
        };
        runtime.launch(job).await.map(|_| ()).map_err(|e| ApiError::internal(format!("sandbox: {e}")))
    }

    /// Remove the session's sandbox. The session's own state is the source
    /// of truth, so a runtime that cannot be reached is logged, not raised:
    /// an archive or delete never waits on the sandbox host.
    pub(super) async fn stop_sandbox(&self, id: &SessionId) {
        let Some(runtime) = &self.sandbox else {
            return;
        };
        let Some(handle) = runtime.handle_for(id.as_str()) else {
            return;
        };
        match runtime.stop(&handle).await {
            Ok(()) | Err(RuntimeError::Unknown(_)) => {}
            Err(e) => tracing::warn!(session = %id, error = %e, "sessions: could not remove the sandbox"),
        }
    }
}
