//! A work item's lifecycle as workers drive it: poll, ack, heartbeat, stop.

use std::time::Duration;

use shared::{ApiError, AuthContext, Timestamp};

use crate::board::Entry;
use crate::service::{not_found, WorkScope, WorkService, DEFAULT_TTL, MAX_TTL, RECLAIM};
use crate::tokens::WorkClaims;
use crate::types::{Heartbeat, HeartbeatTag, Work, WorkState};

impl WorkService {
    /// `GET …/work/poll`: hand out the oldest queued work of the environment,
    /// with its secret, waiting up to `block` for some.
    pub async fn poll(&self, ctx: &AuthContext, env: &str, block: Duration) -> Option<Work> {
        let board = self.board();
        let deadline = tokio::time::Instant::now() + block;
        loop {
            let notified = board.queued.notified();
            let now = tokio::time::Instant::now();
            let mut taken = board.each(|e| {
                let free = e.handed_out.is_none_or(|t| now.duration_since(t) > RECLAIM);
                (e.workspace == ctx.workspace.as_str()
                    && e.work.environment_id == env
                    && !e.hosted
                    && e.work.state == WorkState::Queued
                    && free)
                    .then(|| e.work.id.clone())
            });
            taken.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            if let Some(id) = taken.first() {
                let claims = board.with(id, |e| {
                    e.handed_out = Some(now);
                    WorkClaims {
                        work: e.work.id.as_str().to_owned(),
                        session: e.work.data.id.clone(),
                        ws: ctx.workspace.as_str().to_owned(),
                        org: ctx.org.as_str().to_owned(),
                    }
                })?;
                let mut work = board.with(id, |e| e.work.clone())?;
                work.secret = Some(self.tokens().secret(&claims));
                return Some(work);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return None;
            }
        }
    }

    /// `POST …/work/{id}/ack`: a worker took it.
    pub fn ack(
        &self,
        ctx: &AuthContext,
        scope: Option<&WorkScope>,
        env: &str,
        raw: &str,
    ) -> Result<Work, ApiError> {
        let board = self.board();
        let id = self.find(ctx, scope, env, raw)?;
        board
            .with(&id, |e| {
                if e.work.state == WorkState::Queued {
                    e.work.state = WorkState::Starting;
                    e.work.acknowledged_at = Some(Timestamp::now());
                }
                e.work.clone()
            })
            .ok_or_else(not_found)
    }

    /// `POST …/work/{id}/heartbeat`: extend the lease, conditional on the
    /// last heartbeat the worker saw (`NO_HEARTBEAT` claims a fresh lease).
    pub fn heartbeat(
        &self,
        ctx: &AuthContext,
        scope: Option<&WorkScope>,
        env: &str,
        raw: &str,
        expected: Option<&str>,
        ttl: Option<u64>,
    ) -> Result<Heartbeat, ApiError> {
        let board = self.board();
        let id = self.find(ctx, scope, env, raw)?;
        let ttl = ttl.filter(|t| *t > 0).unwrap_or(DEFAULT_TTL).min(MAX_TTL);
        board.with(&id, |e| heartbeat(e, expected, ttl)).ok_or_else(not_found)?
    }

    /// `POST …/work/{id}/stop`: graceful (`stopping`) unless `force`.
    pub fn stop(
        &self,
        ctx: &AuthContext,
        scope: Option<&WorkScope>,
        env: &str,
        raw: &str,
        force: bool,
    ) -> Result<Work, ApiError> {
        let board = self.board();
        let id = self.find(ctx, scope, env, raw)?;
        board
            .with(&id, |e| {
                let now = Timestamp::now();
                e.work.stop_requested_at.get_or_insert(now);
                if force || e.work.state == WorkState::Queued {
                    e.work.state = WorkState::Stopped;
                    e.work.stopped_at = Some(now);
                } else if e.work.state != WorkState::Stopped {
                    e.work.state = WorkState::Stopping;
                }
                e.work.clone()
            })
            .ok_or_else(not_found)
    }
}

fn heartbeat(e: &mut Entry, expected: Option<&str>, ttl: u64) -> Result<Heartbeat, ApiError> {
    let current = e.last_heartbeat.clone().unwrap_or_else(|| "NO_HEARTBEAT".into());
    if let Some(expected) = expected {
        if expected != current {
            let mut err = ApiError::invalid_request("expected_last_heartbeat does not match the lease");
            err.status = 412;
            err.extra.insert(
                "details".into(),
                serde_json::json!({"current_state": {"state": e.work.state, "ttl_seconds": ttl, "last_heartbeat": e.last_heartbeat.clone().unwrap_or_default()}}),
            );
            return Err(err);
        }
    }
    let alive = !matches!(e.work.state, WorkState::Stopping | WorkState::Stopped);
    if alive {
        let now = Timestamp::now();
        e.work.latest_heartbeat_at = Some(now);
        e.work.started_at.get_or_insert(now);
        e.work.state = WorkState::Active;
        e.last_heartbeat = Some(now.to_wire());
        e.lease_until = Some(tokio::time::Instant::now() + Duration::from_secs(ttl));
    }
    Ok(Heartbeat {
        last_heartbeat: e.last_heartbeat.clone().unwrap_or_default(),
        lease_extended: alive,
        state: e.work.state,
        ttl_seconds: ttl,
        tag: HeartbeatTag::WorkHeartbeat,
    })
}
