//! One delivery of a fire from an external scheduler, whatever carries it:
//! the same inbox, the same logging and the same answer to the feed.

use chrono::{DateTime, Utc};

use super::{Claim, FireError, FireRegistry};
use crate::ports::ScheduleSpec;

/// Run `fire` through `registry`; `true` once it needs no more delivery: it
/// ran, needs no run, or can never run (it does not parse, or no handler owns
/// its kind). A fire another replica holds, or one that failed, is delivered
/// again. `id` names the delivery in the log.
pub async fn deliver(registry: &FireRegistry, fire: Option<(ScheduleSpec, DateTime<Utc>)>, id: &str) -> bool {
    let Some((spec, at)) = fire else {
        tracing::error!(%id, "fires: a delivery carries no fire; dropped");
        return true;
    };
    match registry.fire(&spec, at).await {
        Ok(Claim::Run | Claim::Done) => true,
        Ok(Claim::Held) => false,
        Err(FireError::NoHandler(kind)) => {
            tracing::error!(target = %spec.target.key(), %kind, "fires: no handler for the fire's kind; dropped");
            true
        }
        Err(e) => {
            tracing::warn!(target = %spec.target.key(), %at, error = %e, "fires: a fire failed; it runs again after its lease");
            false
        }
    }
}
