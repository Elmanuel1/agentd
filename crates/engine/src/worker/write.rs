//! A step's writes, each in one transaction: the opening before its model
//! call or each MCP result, and the commit that ends it. A thread's step
//! changes the session's row only to record a grader's verdict; the session's
//! own step writes its status and usage. Every replica's streams are told
//! after each.

use sessions::signals::Signal;
use sessions::steps::{Claim, SessionChange, StatusDeliveries, StepEnd, StepWrite};
use sessions::types::{OutcomeEvaluation, Session, SessionStatus, ThreadId};
use shared::BackendError;
use webhooks::EventKind;

use super::Loop;
use super::turn::Batch;

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
        let change = to.is_some().then_some(&change as &SessionChange<'_>);
        self.land(claim, b, change, Landing::Write(self.settings.lease_seconds()), &[]).await
    }

    /// End a thread's step, recording `verdict` when its grader gave one.
    pub(super) async fn commit(
        &self,
        claim: &Claim,
        end: StepEnd,
        b: &Batch,
        verdict: Option<&OutcomeEvaluation>,
    ) -> Result<bool, BackendError> {
        let change = move |s: &mut Session| {
            let Some(v) = verdict else { return false };
            s.outcome_evaluations.retain(|e| e.outcome_id != v.outcome_id);
            s.outcome_evaluations.push(v.clone());
            true
        };
        let change = verdict.is_some().then_some(&change as &SessionChange<'_>);
        self.land(claim, b, change, Landing::Commit(end), &[]).await
    }

    /// End the session's own step with `session`'s status and usage, giving
    /// each of `wake` a step.
    pub(super) async fn settled(
        &self,
        claim: &Claim,
        end: StepEnd,
        b: &Batch,
        session: &Session,
        wake: &[ThreadId],
    ) -> Result<bool, BackendError> {
        let after = session.clone();
        let change = move |s: &mut Session| {
            let same = s.status == after.status && s.usage == after.usage;
            s.status = after.status;
            s.usage = after.usage.clone();
            !same
        };
        self.land(claim, b, Some(&change as &SessionChange<'_>), Landing::Commit(end), wake).await
    }

    /// Land `b` with `change` as `how` says; `false` when another worker
    /// took the step over, with nothing written.
    async fn land(
        &self,
        claim: &Claim,
        b: &Batch,
        change: Option<&SessionChange<'_>>,
        how: Landing,
        wake: &[ThreadId],
    ) -> Result<bool, BackendError> {
        let hooks = &self.stores.webhooks;
        let (ws, id) = (claim.workspace.clone(), claim.session.clone());
        let deliveries: Option<Box<StatusDeliveries<'_>>> = (!hooks.is_empty()).then(|| {
            Box::new(move |org: &str, status: SessionStatus| {
                let kind = match status {
                    SessionStatus::Running => EventKind::SessionRunStarted,
                    SessionStatus::Idle => EventKind::SessionIdled,
                    SessionStatus::Terminated => EventKind::SessionTerminated,
                    SessionStatus::Rescheduling => return Vec::new(),
                };
                hooks.deliveries(org, ws.as_str(), kind, id.as_str())
            }) as Box<StatusDeliveries<'_>>
        });
        let w = StepWrite {
            read: &b.read,
            at: b.at,
            events: &b.events,
            engine: &b.engine,
            session: change,
            deliveries: deliveries.as_deref(),
            wake,
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
}
