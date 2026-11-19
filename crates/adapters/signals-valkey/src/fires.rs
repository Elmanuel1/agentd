//! Fires an external scheduler (such as a Temporal worker) delivers: each
//! entry of `<prefix>.fires` has the fields `spec` (the schedule's
//! [`ScheduleSpec`] as JSON) and `scheduled_at` (RFC 3339), and runs through
//! the [`FireRegistry`] into the fire queue, which keeps one row for a fire delivered twice.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use redis::streams::StreamId;
use shared::ports::ScheduleSpec;
use shared::ports::fire::{self, FireRegistry};

use crate::changes::NativeConsumer;

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

impl NativeConsumer for Fires {
    fn ordered(&self) -> bool {
        false
    }

    async fn consume(&self, entry: &StreamId) -> bool {
        fire::deliver(&self.registry, parse(entry), &entry.id).await
    }
}
