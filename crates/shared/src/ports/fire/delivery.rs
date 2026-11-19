//! One delivery of a fire from an external scheduler, whatever carries it:
//! the same queue, the same logging and the same answer to the feed.

use chrono::{DateTime, Utc};

use super::FireRegistry;
use crate::ports::ScheduleSpec;

/// Write `fire` to `registry`'s fire queue; `true` once it needs no more
/// delivery: its occurrence is queued, or it carries none. A write that
/// failed is delivered again. `id` names the delivery in the log.
pub async fn deliver(registry: &FireRegistry, fire: Option<(ScheduleSpec, DateTime<Utc>)>, id: &str) -> bool {
    let Some((spec, at)) = fire else {
        tracing::error!(%id, "fires: a delivery carries no fire; dropped");
        return true;
    };
    match registry.enqueue(&spec, at).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(target = %spec.target.key(), %at, error = %e, "fires: a fire cannot be queued; delivered again");
            false
        }
    }
}
