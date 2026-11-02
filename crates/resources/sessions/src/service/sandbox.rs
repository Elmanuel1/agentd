use shared::ports::{JobKind, JobSpec, RuntimeError};
use shared::{ApiError, AuthContext, ErrorType};

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
            env: self
                .work
                .as_ref()
                .map(|w| w.issue(ctx, &session.environment_id, &session.id))
                .unwrap_or_default(),
        };
        match runtime.launch(job).await {
            Ok(_) => Ok(()),
            // A daemon that cannot be reached is transient: 529, which the
            // SDKs retry.
            Err(e @ RuntimeError::Unavailable(_)) => Err(ApiError {
                status: ErrorType::Overloaded.status(),
                error_type: ErrorType::Overloaded,
                message: format!("sandbox: {e}"),
                extra: serde_json::Map::new(),
            }),
            Err(e) => Err(ApiError::internal(format!("sandbox: {e}"))),
        }
    }

    /// Remove the session's sandbox (on archive, delete and terminate). The session's own state is the source
    /// of truth, so a runtime that cannot be reached is logged, not raised:
    /// an archive or delete never waits on the sandbox host.
    pub(super) async fn stop_sandbox(&self, id: &SessionId) {
        if let Some(work) = &self.work {
            work.end(id);
        }
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
