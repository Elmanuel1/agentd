//! `agentd.retention.last_success`: when this replica last completed an
//! event retention run, in Unix seconds, so an alarm sees retention stall.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use shared::Timestamp;

/// Where a replica records its last completed run.
#[derive(Debug, Clone, Default)]
pub struct LastSuccess(Arc<AtomicI64>);

impl LastSuccess {
    /// A run completed at `at`.
    pub fn record(&self, at: Timestamp) {
        self.0.store(at.as_datetime().timestamp(), Ordering::Relaxed);
    }
}

/// The gauge, read from what the runs record; 0 until the first run.
pub fn last_success(meter: &opentelemetry::metrics::Meter) -> LastSuccess {
    let last = LastSuccess::default();
    let read = last.0.clone();
    meter
        .i64_observable_gauge("agentd.retention.last_success")
        .with_unit("s")
        .with_description("When this replica last completed an event retention run")
        .with_callback(move |o| o.observe(read.load(Ordering::Relaxed), &[]))
        .build();
    last
}
