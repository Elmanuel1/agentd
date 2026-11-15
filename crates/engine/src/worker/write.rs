//! A step's two writes, each in one transaction under the session's append
//! lock: the opening before its model call, and the commit that ends it.
//! Every replica's streams are told after each.

use sessions::signals::Signal;
use sessions::steps::{Claim, StepEnd, StepWrite};
use sessions::types::{Session, SessionStatus};
use shared::BackendError;

use super::turn::Batch;
use super::Loop;

impl Loop {
    pub(super) async fn write(
        &self,
        claim: &Claim,
        b: &Batch,
        to: Option<SessionStatus>,
    ) -> Result<bool, BackendError> {
        let change = move |s: &mut Session| to.filter(|t| s.status != *t).map(|t| s.status = t).is_some();
        let w = StepWrite { read: &b.read, at: b.at, events: &b.events, engine: &b.engine, session: &change };
        crate::metrics::appended(&b.events);
        let wrote = self
            .stores
            .steps
            .write(claim, self.settings.lease_seconds(), w)
            .await
            .map_err(BackendError::new)?;
        self.publish(&Signal { workspace: claim.workspace.clone(), session: claim.session.clone() }).await;
        Ok(wrote)
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
        let w = StepWrite { read: &b.read, at: b.at, events: &b.events, engine: &b.engine, session: &change };
        crate::metrics::appended(&b.events);
        let done = self.stores.steps.commit(claim, end, w).await.map_err(BackendError::new)?;
        self.publish(&Signal { workspace: claim.workspace.clone(), session: claim.session.clone() }).await;
        Ok(done)
    }
}
