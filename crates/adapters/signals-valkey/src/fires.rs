//! Fires an external scheduler (such as a Temporal worker) delivers: each
//! entry of `<prefix>.fires` has the fields `spec` (the schedule's
//! [`ScheduleSpec`] as JSON) and `scheduled_at` (RFC 3339), and runs through
//! the [`FireRegistry`], whose inbox drops a fire delivered twice.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use redis::streams::StreamId;
use shared::ports::fire::{Claim, FireError, FireRegistry};
use shared::ports::ScheduleSpec;

use crate::changes::Consumer;

/// Runs the fires of the stream.
pub(crate) struct Fires {
    registry: Arc<FireRegistry>,
}

impl Fires {
    pub(crate) fn new(registry: Arc<FireRegistry>) -> Self {
        Self { registry }
    }
}

/// The fire an entry carries, when it carries one.
fn parse(entry: &StreamId) -> Option<(ScheduleSpec, DateTime<Utc>)> {
    let spec: String = entry.get("spec")?;
    let at: String = entry.get("scheduled_at")?;
    let spec = serde_json::from_str(&spec).ok()?;
    let at = DateTime::parse_from_rfc3339(&at).ok()?;
    Some((spec, at.with_timezone(&Utc)))
}

#[async_trait]
impl Consumer for Fires {
    fn ordered(&self) -> bool {
        false
    }

    /// Done once the fire ran or needs no run; a fire another replica holds,
    /// or one that failed, stays pending and is delivered again.
    async fn consume(&self, entry: &StreamId) -> bool {
        let Some((spec, at)) = parse(entry) else {
            tracing::error!(id = %entry.id, "fires: an entry carries no fire; dropped");
            return true;
        };
        match self.registry.fire(&spec, at).await {
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
}
