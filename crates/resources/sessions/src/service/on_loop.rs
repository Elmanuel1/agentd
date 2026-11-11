//! Which sessions the loop serves, and how a session started on the harness
//! path moves to it (L7, L-19).

use shared::{ApiError, AuthContext};

use super::SessionsService;
use crate::store::Wake;
use crate::types::{Session, SessionStatus};

impl SessionsService {
    /// Whether a session of `ctx`'s workspace created now runs on the loop.
    pub(super) fn starts_on_loop(&self, ctx: &AuthContext, session: &Session) -> bool {
        self.on_loop.joined(&ctx.workspace).is_some_and(|joined| session.created_at >= joined)
    }

    /// How a client event for `session` reaches the loop. A session the
    /// harness started, in a listed workspace, moves at its next idle turn:
    /// its sandbox stops first, and nothing is appended when it cannot.
    pub(super) async fn wake_for(&self, ctx: &AuthContext, session: &Session) -> Result<Wake, ApiError> {
        let Some(joined) = self.on_loop.joined(&ctx.workspace) else {
            return Ok(Wake::No);
        };
        if session.created_at >= joined {
            return Ok(Wake::Step);
        }
        // Running: the harness, or the loop after an earlier move, serves the turn.
        if session.status != SessionStatus::Idle {
            return Ok(Wake::IfQueued);
        }
        self.halt_sandbox(&session.id).await?;
        if let Some(work) = &self.work {
            let environment = self.deps.resolve_environment(ctx, &session.environment_id).await?;
            work.queue(ctx, session, &environment).await?;
        }
        Ok(Wake::Step)
    }
}
