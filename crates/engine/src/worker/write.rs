//! A step's two writes, each in one transaction under the session's append
//! lock: the opening before its model call, and the commit that ends it.
//! Every replica's streams are told after each.

use sessions::signals::Signal;
use sessions::steps::{Claim, SessionChange, StatusDeliveries, StepEnd, StepWrite};
use sessions::types::{Session, SessionStatus};
use shared::BackendError;
use webhooks::EventKind;

use super::turn::Batch;
use super::Loop;

/// How a step's write lands: the opening, keeping its lease, or the commit.
enum Landing {
    Write(i64),
    Commit(StepEnd),
}

impl Loop {
    pub(super) async fn write(
        &self,
        claim: &Claim,
        b: &Batch,
        to: Option<SessionStatus>,
    ) -> Result<bool, BackendError> {
        let change = move |s: &mut Session| to.filter(|t| s.status != *t).map(|t| s.status = t).is_some();
        self.land(claim, b, &change, Landing::Write(self.settings.lease_seconds())).await
    }

    pub(super) async fn commit(
        &self,
        claim: &Claim,
        end: StepEnd,
        b: &Batch,
        session: &Session,
    ) -> Result<bool, BackendError> {
        let after = session.clone();
        let change = move |s: &mut Session| {
            s.status = after.status;
            s.usage = after.usage.clone();
            s.outcome_evaluations = after.outcome_evaluations.clone();
            true
        };
        self.land(claim, b, &change, Landing::Commit(end)).await
    }

    /// Land `b` with `change` as `how` says; `false` when another worker
    /// took the step over, with nothing written.
    async fn land(
        &self,
        claim: &Claim,
        b: &Batch,
        change: &SessionChange<'_>,
        how: Landing,
    ) -> Result<bool, BackendError> {
        let deliveries = self.deliveries(claim);
        let w = StepWrite {
            read: &b.read,
            at: b.at,
            events: &b.events,
            engine: &b.engine,
            session: change,
            deliveries: deliveries.as_deref(),
        };
        crate::metrics::appended(&b.events);
        let steps = &self.stores.steps;
        let landed = match how {
            Landing::Write(lease) => steps.write(claim, lease, w).await,
            Landing::Commit(end) => steps.commit(claim, end, w).await,
        };
        let landed = landed.map_err(BackendError::new)?;
        self.publish(&Signal { workspace: claim.workspace.clone(), session: claim.session.clone() }).await;
        Ok(landed)
    }

    /// The deliveries a status change of `claim`'s session sends, when any
    /// endpoint is configured (H-1).
    fn deliveries(&self, claim: &Claim) -> Option<Box<StatusDeliveries<'_>>> {
        let hooks = &self.stores.webhooks;
        if hooks.is_empty() {
            return None;
        }
        let (ws, id) = (claim.workspace.clone(), claim.session.clone());
        Some(Box::new(move |org: &str, status: SessionStatus| {
            let kind = match status {
                SessionStatus::Running => EventKind::SessionRunStarted,
                SessionStatus::Idle => EventKind::SessionIdled,
                SessionStatus::Terminated => EventKind::SessionTerminated,
                SessionStatus::Rescheduling => return Vec::new(),
            };
            hooks.deliveries(org, ws.as_str(), kind, id.as_str())
        }))
    }
}
