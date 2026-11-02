//! The work API's rules: who may touch a work item, its lifecycle
//! (queued, starting, active, stopping, stopped) and its heartbeat lease.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use shared::{ApiError, AuthContext, Timestamp};

use crate::board::{Board, Entry};
use crate::tokens::{WorkClaims, WorkTokens};
use crate::types::{QueueStats, StatsTag, Work, WorkData, WorkDataTag, WorkId, WorkState, WorkTag};

/// The lease a heartbeat grants when the worker asks for no particular TTL.
pub const DEFAULT_TTL: u64 = 90;
/// The longest lease a heartbeat grants.
pub const MAX_TTL: u64 = 600;
/// A handed-out item no one acknowledged goes back to the queue after this.
pub(crate) const RECLAIM: Duration = Duration::from_millis(5000);

/// Set by the edge when the caller authenticated with a work token: the
/// caller may touch that work item only.
#[derive(Debug, Clone)]
pub struct WorkScope(pub WorkClaims);

/// Work items and their sandbox operations.
#[derive(Debug)]
pub struct WorkService {
    board: Arc<Board>,
    tokens: WorkTokens,
}

pub(crate) fn not_found() -> ApiError {
    ApiError::not_found("work not found")
}

impl WorkService {
    /// Sign work tokens with `tokens`.
    pub fn new(tokens: WorkTokens) -> Self {
        Self { board: Arc::new(Board::default()), tokens }
    }

    /// The board, shared with the remote sandbox.
    pub fn board(&self) -> Arc<Board> {
        self.board.clone()
    }

    /// The token signer, shared with the edge.
    pub fn tokens(&self) -> &WorkTokens {
        &self.tokens
    }

    /// Queue the work of running `session` in `environment_id`; the work and
    /// its secret. Hosted work is taken by its own sandbox's worker.
    pub fn issue(
        &self,
        ctx: &AuthContext,
        environment_id: &str,
        session: &str,
        hosted: bool,
    ) -> (Work, String) {
        let work = Work {
            id: WorkId::generate(),
            acknowledged_at: None,
            created_at: Timestamp::now(),
            data: WorkData { id: session.to_owned(), tag: WorkDataTag::Session },
            environment_id: environment_id.to_owned(),
            latest_heartbeat_at: None,
            metadata: BTreeMap::new(),
            secret: None,
            started_at: None,
            state: WorkState::Queued,
            stop_requested_at: None,
            stopped_at: None,
            tag: WorkTag::Work,
        };
        let claims = WorkClaims {
            work: work.id.as_str().to_owned(),
            session: session.to_owned(),
            ws: ctx.workspace.as_str().to_owned(),
            org: ctx.org.as_str().to_owned(),
        };
        let secret = self.tokens.secret(&claims);
        self.board.insert(Entry::new(work.clone(), ctx.workspace.as_str().to_owned(), hosted));
        (work, secret)
    }

    /// Forget `session`'s work (its sandbox is gone).
    pub fn end(&self, session: &str) {
        self.board.remove_session(session);
    }

    /// The work item `raw` in `env`, if `ctx` (and `scope`, for a work
    /// token) may see it.
    pub(crate) fn find(
        &self,
        ctx: &AuthContext,
        scope: Option<&WorkScope>,
        env: &str,
        raw: &str,
    ) -> Result<WorkId, ApiError> {
        let id = WorkId::parse(raw).map_err(|_| not_found())?;
        if scope.is_some_and(|s| s.0.work != raw) {
            return Err(not_found());
        }
        let visible =
            self.board.with(&id, |e| e.workspace == ctx.workspace.as_str() && e.work.environment_id == env);
        visible.filter(|v| *v).map(|_| id).ok_or_else(not_found)
    }

    /// `GET …/work/{id}`.
    pub fn retrieve(
        &self,
        ctx: &AuthContext,
        scope: Option<&WorkScope>,
        env: &str,
        raw: &str,
    ) -> Result<Work, ApiError> {
        let id = self.find(ctx, scope, env, raw)?;
        self.board.with(&id, |e| e.work.clone()).ok_or_else(not_found)
    }

    /// `GET …/work`: the environment's work, newest first.
    pub fn list(&self, ctx: &AuthContext, env: &str) -> Vec<Work> {
        let ws = ctx.workspace.as_str();
        let mut all =
            self.board.each(|e| (e.workspace == ws && e.work.environment_id == env).then(|| e.work.clone()));
        all.sort_by(|a, b| b.created_at.as_datetime().cmp(&a.created_at.as_datetime()));
        all
    }

    /// `POST …/work/{id}`: merge metadata (`null` deletes a key).
    pub fn update(
        &self,
        ctx: &AuthContext,
        env: &str,
        raw: &str,
        patch: BTreeMap<String, Option<String>>,
    ) -> Result<Work, ApiError> {
        let id = self.find(ctx, None, env, raw)?;
        self.board
            .with(&id, |e| {
                for (k, v) in patch {
                    match v {
                        Some(v) => e.work.metadata.insert(k, v),
                        None => e.work.metadata.remove(&k),
                    };
                }
                e.work.clone()
            })
            .ok_or_else(not_found)
    }

    /// `GET …/work/stats`.
    pub fn stats(&self, ctx: &AuthContext, env: &str) -> QueueStats {
        let all: Vec<(WorkState, bool, Timestamp)> = self.board.each(|e| {
            (e.workspace == ctx.workspace.as_str() && e.work.environment_id == env && !e.hosted)
                .then(|| (e.work.state, e.handed_out.is_some(), e.work.created_at))
        });
        let queued: Vec<_> = all.iter().filter(|(s, ..)| *s == WorkState::Queued).collect();
        QueueStats {
            depth: queued.iter().filter(|(_, out, _)| !out).count() as u64,
            oldest_queued_at: queued.iter().map(|(.., t)| *t).min_by_key(Timestamp::as_datetime),
            pending: queued.iter().filter(|(_, out, _)| *out).count() as u64,
            tag: StatsTag::WorkQueueStats,
            workers_polling: None,
        }
    }
}
