//! A work item's lifecycle as workers drive it: poll, ack, heartbeat, stop.
//! A `stopping` item whose lease ran out is served as `stopped`
//! ([`crate::WorkItem::served`]).

use std::collections::BTreeMap;
use std::time::Duration;

use shared::{ApiError, AuthContext, Timestamp, WorkspaceId};

use crate::caller::WorkCaller;
use crate::service::{backend, not_found, WorkService};
use crate::store::{Beat, Reach, RUNTIME};
use crate::tokens::WorkClaims;
use crate::types::{Heartbeat, HeartbeatTag, Work, WorkState};

/// The lease a heartbeat grants when it asks for none, or for 0, as on live;
/// an acknowledgement holds the item this long before its first heartbeat.
pub const DEFAULT_TTL: i64 = 300;
/// A handed-out item no one acknowledged is handed out again after this.
pub const RECLAIM: Duration = Duration::from_millis(5000);

fn after(at: Timestamp, seconds: i64) -> Timestamp {
    Timestamp::from_datetime(at.as_datetime() + chrono::Duration::seconds(seconds))
}

impl WorkService {
    /// `GET …/work/poll`: hand out the oldest queued item of the caller's
    /// queue with its secret, waiting `block` for one when there is none.
    pub async fn poll(
        &self,
        caller: &WorkCaller,
        block: Duration,
        reclaim: Duration,
    ) -> Result<Option<Work>, ApiError> {
        let WorkCaller::Queue { queue, .. } = caller else { return Err(not_found()) };
        let mut claimed = self.store.claim(queue, reclaim, Timestamp::now()).await.map_err(backend)?;
        if claimed.is_none() && !block.is_zero() {
            tokio::time::sleep(block).await;
            claimed = self.store.claim(queue, reclaim, Timestamp::now()).await.map_err(backend)?;
        }
        let Some(item) = claimed else { return Ok(None) };
        let ctx = AuthContext {
            org: item.org.clone(),
            workspace: item.workspace.clone(),
            api_key_id: String::new(),
        };
        let variables = match (&self.cloud, item.queue == RUNTIME) {
            (Some(cloud), true) => cloud.of(&ctx, &item.work.id).await?,
            _ => BTreeMap::new(),
        };
        let claims = WorkClaims {
            session: item.work.id.clone(),
            ws: item.workspace.as_str().to_owned(),
            org: item.org.as_str().to_owned(),
        };
        let mut work = item.work;
        work.secret = Some(self.tokens.secret(&claims, &self.api_base_url, &variables));
        Ok(Some(work))
    }

    /// `POST …/work/{id}/ack`: a worker took it.
    pub async fn ack(&self, caller: &WorkCaller, env: &str, id: &str) -> Result<Work, ApiError> {
        let (ws, reach) = self.target(caller, env, id)?;
        let now = Timestamp::now();
        let item = self.store.ack(&ws, id, &reach, now, after(now, DEFAULT_TTL)).await.map_err(backend)?;
        item.map(|i| i.work).ok_or_else(not_found)
    }

    /// `POST …/work/{id}/heartbeat`: extend the lease, conditional on the
    /// last heartbeat the worker saw (`NO_HEARTBEAT` claims a fresh lease).
    /// The TTL is echoed as given; none, or 0, grants 300 s.
    pub async fn heartbeat(
        &self,
        caller: &WorkCaller,
        env: &str,
        id: &str,
        expected: Option<&str>,
        ttl: Option<i64>,
    ) -> Result<Heartbeat, ApiError> {
        let (ws, reach) = self.target(caller, env, id)?;
        let ttl = ttl.filter(|t| *t != 0).unwrap_or(DEFAULT_TTL);
        let at = Timestamp::now();
        let beat = Beat {
            expected: expected.map(str::to_owned),
            stamp: at.to_wire(),
            at,
            lease_until: after(at, ttl),
        };
        let (item, extended) =
            self.store.heartbeat(&ws, id, &reach, &beat).await.map_err(backend)?.ok_or_else(not_found)?;
        let ending = matches!(item.work.state, WorkState::Stopping | WorkState::Stopped);
        if !extended && !ending {
            let mut err = ApiError::invalid_request("expected_last_heartbeat does not match the lease");
            err.status = 412;
            err.extra.insert(
                "details".into(),
                serde_json::json!({"current_state": {"state": item.work.state, "ttl_seconds": ttl, "last_heartbeat": item.last_heartbeat.clone().unwrap_or_default()}}),
            );
            return Err(err);
        }
        Ok(Heartbeat {
            last_heartbeat: item.last_heartbeat.unwrap_or_default(),
            lease_extended: extended,
            state: item.work.state,
            ttl_seconds: ttl,
            tag: HeartbeatTag::WorkHeartbeat,
        })
    }

    /// `POST …/work/{id}/stop`: graceful (`stopping`) unless `force`, or
    /// unless no worker holds it.
    pub async fn stop(
        &self,
        caller: &WorkCaller,
        env: &str,
        id: &str,
        force: bool,
    ) -> Result<Work, ApiError> {
        let (ws, reach) = self.target(caller, env, id)?;
        let item = self.store.stop(&ws, id, &reach, force, Timestamp::now()).await.map_err(backend)?;
        item.map(|i| i.work).ok_or_else(not_found)
    }

    /// The session was archived, deleted or terminated: its item stops, and
    /// is kept.
    pub async fn end_session(&self, ws: &WorkspaceId, session: &str) -> Result<(), ApiError> {
        self.store.stop(ws, session, &Reach::default(), false, Timestamp::now()).await.map_err(backend)?;
        Ok(())
    }
}
