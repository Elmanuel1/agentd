use shared::ports::{JobKind, JobSpec, RuntimeError};
use shared::{ApiError, AuthContext, ErrorType, WorkspaceId};

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
        if self.starts_on_loop(ctx, session) {
            // The loop serves it: its work waits on its queue for a worker.
            return match &self.work {
                Some(work) => work.queue(ctx, session, &environment).await,
                None => Ok(()),
            };
        }
        let Some(runtime) = &self.sandbox else {
            return Ok(());
        };
        let env = match &self.work {
            Some(w) => w.issue(ctx, session, &environment).await?,
            None => Default::default(),
        };
        let job = JobSpec {
            kind: JobKind::Session,
            target_id: session.id.to_string(),
            workspace_id: ctx.workspace.as_str().to_owned(),
            environment_id: session.environment_id.clone(),
            environment,
            env,
        };
        let launched = runtime.launch(job).await;
        if launched.is_err() {
            if let Some(work) = &self.work {
                work.end(&ctx.workspace, &session.id).await;
            }
        }
        launched.map(drop).map_err(sandbox_error)
    }

    /// Stop the harness sandbox of a session moving to the loop, answering
    /// its error: a sandbox that is already gone is stopped.
    pub(super) async fn halt_sandbox(&self, id: &SessionId) -> Result<(), ApiError> {
        let Some(runtime) = &self.sandbox else {
            return Ok(());
        };
        let Some(handle) = runtime.handle_for(id.as_str()) else {
            return Ok(());
        };
        match runtime.stop(&handle).await {
            Ok(()) | Err(RuntimeError::Unknown(_)) => Ok(()),
            Err(e) => Err(sandbox_error(e)),
        }
    }

    /// End the session's sandbox (on archive and terminate): its work item
    /// stops, then the sandbox goes.
    pub(super) async fn stop_sandbox(&self, ws: &WorkspaceId, id: &SessionId) {
        if let Some(work) = &self.work {
            work.end(ws, id).await;
        }
        self.remove_runtime(id).await;
    }

    /// Remove a deleted session's sandbox: its work item stopped with the
    /// delete.
    pub(super) async fn remove_sandbox(&self, id: &SessionId) {
        if let Some(work) = &self.work {
            work.forget(id);
        }
        self.remove_runtime(id).await;
    }

    /// The session's own state is the source of truth, so a runtime that
    /// cannot be reached is logged, not raised: an archive or delete never
    /// waits on the sandbox host.
    async fn remove_runtime(&self, id: &SessionId) {
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

/// A daemon that cannot be reached is transient: 529, which the SDKs retry.
fn sandbox_error(e: RuntimeError) -> ApiError {
    match e {
        RuntimeError::Unavailable(_) => ApiError::of(ErrorType::Overloaded, format!("sandbox: {e}")),
        e => ApiError::internal(format!("sandbox: {e}")),
    }
}
