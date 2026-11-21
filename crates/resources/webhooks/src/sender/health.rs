//! What this replica knows of the endpoints' shared states, read every 10 s.
//! A read counts from when it started, since its rows can be no newer; one
//! that completes 11 s or more after it started is discarded, and while the
//! last good read is that old the replica starts no POST (SH-34).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use shared::BackendError;
use tokio::time::Instant;

use crate::states::{Configured, DisabledReason, EndpointStates};

/// The oldest a state read may be for a POST to start.
pub(crate) const FRESH: Duration = Duration::from_secs(11);

#[derive(Debug, Default)]
struct Snapshot {
    /// When the read started; none before the first good read.
    read_at: Option<Instant>,
    disabled: HashMap<String, DisabledReason>,
}

/// The endpoint states as last read, with the disables this replica made since.
pub(crate) struct Health {
    states: Arc<dyn EndpointStates>,
    snapshot: Mutex<Snapshot>,
}

impl Health {
    pub(crate) fn new(states: Arc<dyn EndpointStates>) -> Self {
        Self { states, snapshot: Mutex::default() }
    }

    pub(crate) fn states(&self) -> &dyn EndpointStates {
        self.states.as_ref()
    }

    fn snapshot(&self) -> std::sync::MutexGuard<'_, Snapshot> {
        self.snapshot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Read the shared states.
    pub(crate) async fn refresh(&self) -> Result<(), BackendError> {
        let started = Instant::now();
        let rows = self.states.read().await?;
        if started.elapsed() >= FRESH {
            return Err(BackendError::new(std::io::Error::other(
                "the endpoint state read ran late; discarded",
            )));
        }
        let disabled = rows.into_iter().filter_map(|r| Some((r.endpoint_key, r.disabled?))).collect();
        *self.snapshot() = Snapshot { read_at: Some(started), disabled };
        Ok(())
    }

    /// Whether the last good read is recent enough for a POST to start.
    pub(crate) fn fresh(&self) -> bool {
        self.snapshot().read_at.is_some_and(|at| at.elapsed() < FRESH)
    }

    pub(crate) fn disabled(&self, key: &str) -> bool {
        self.snapshot().disabled.contains_key(key)
    }

    /// The disabled endpoints by reason, for the gauge.
    pub(crate) fn disabled_by_reason(&self) -> HashMap<DisabledReason, usize> {
        let mut counts = HashMap::new();
        for reason in self.snapshot().disabled.values() {
            *counts.entry(*reason).or_insert(0) += 1;
        }
        counts
    }

    /// Disable `endpoint` for `why` everywhere, and here at once; `true` when this disabled it.
    pub(crate) async fn disable(
        &self,
        endpoint: &Configured,
        why: DisabledReason,
    ) -> Result<bool, BackendError> {
        self.snapshot().disabled.insert(endpoint.endpoint_key.clone(), why);
        self.states.disable(endpoint, why).await
    }

    /// A failed attempt; `true` when it disabled the endpoint, after 120 h of failures.
    pub(crate) async fn failed(&self, endpoint: &Configured) -> Result<bool, BackendError> {
        let disabled = self.states.failed(endpoint).await?;
        if disabled {
            self.snapshot().disabled.insert(endpoint.endpoint_key.clone(), DisabledReason::Failing);
        }
        Ok(disabled)
    }
}
