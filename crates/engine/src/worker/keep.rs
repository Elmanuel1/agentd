//! A step keeps its claim while it runs: the lease is renewed every third of
//! its length, so slow MCP or model work never outlasts it (S-2), and a step
//! whose claim is gone stops where it is. A crashed worker renews nothing, so
//! its step is taken over after the lease (S-5).

use std::future::Future;

use sessions::steps::{Claim, StepWrite};
use sessions::types::Session;
use shared::{BackendError, Timestamp};

use super::Loop;

impl Loop {
    /// Run `step` under `claim`, renewing the claim's lease until it ends.
    pub(super) async fn kept(
        &self,
        claim: &Claim,
        step: impl Future<Output = Result<(), BackendError>>,
    ) -> Result<(), BackendError> {
        let (every, lease) = (self.settings.lease / 3, self.settings.lease_seconds());
        let unchanged = |_: &mut Session| false;
        let renew = async {
            loop {
                tokio::time::sleep(every).await;
                let w = StepWrite {
                    read: &[],
                    at: Timestamp::now(),
                    events: &[],
                    engine: &[],
                    session: &unchanged,
                    deliveries: None,
                    wake: &[],
                };
                match self.stores.steps.write(claim, lease, w).await {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(e) => {
                        tracing::warn!(session = %claim.session, error = %e, "loop: could not renew the step's lease");
                    }
                }
            }
        };
        tokio::select! {
            biased;
            done = step => done,
            () = renew => {
                tracing::info!(session = %claim.session, "loop: the step's claim is gone; it stops");
                Ok(())
            }
        }
    }
}
