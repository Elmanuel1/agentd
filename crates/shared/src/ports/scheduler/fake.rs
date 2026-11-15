//! An in-memory scheduler, test support usable by every crate.

use async_trait::async_trait;

use std::collections::BTreeMap;

use super::*;

/// Deterministic in-memory [`SchedulerPort`] for tests. Records registrations;
/// tests trigger fires manually.
#[derive(Debug, Default)]
pub struct FakeScheduler {
    inner: std::sync::Mutex<BTreeMap<String, (ScheduleSpec, ScheduleState)>>,
}

impl FakeScheduler {
    /// New empty fake.
    pub fn new() -> Self {
        Self::default()
    }

    /// Test hook: all registered specs.
    pub fn registered(&self) -> Vec<ScheduleSpec> {
        self.inner.lock().map(|m| m.values().map(|(s, _)| s.clone()).collect()).unwrap_or_default()
    }
}

#[async_trait]
impl SchedulerPort for FakeScheduler {
    async fn upsert(&self, spec: ScheduleSpec) -> Result<(), SchedulerError> {
        // Validate through the shared engine so fakes enforce the same rules.
        crate::schedule::CronSchedule {
            kind: crate::schedule::CronKind::Cron,
            expression: spec.cron.clone(),
            timezone: spec.timezone.clone(),
        }
        .compile()
        .map_err(|e| SchedulerError::InvalidSpec(e.to_string()))?;
        let mut m = self.inner.lock().map_err(|e| SchedulerError::Unavailable(e.to_string()))?;
        let key = spec.target.key();
        m.insert(key, (spec, ScheduleState { paused: false }));
        Ok(())
    }

    async fn pause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        self.with_state(target, |st| st.paused = true).await
    }

    async fn unpause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        self.with_state(target, |st| st.paused = false).await
    }

    async fn delete(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let mut m = self.inner.lock().map_err(|e| SchedulerError::Unavailable(e.to_string()))?;
        m.remove(&target.key()).map(|_| ()).ok_or_else(|| SchedulerError::Unknown(target.key()))
    }

    async fn describe(&self, target: &FireTarget) -> Result<ScheduleState, SchedulerError> {
        let m = self.inner.lock().map_err(|e| SchedulerError::Unavailable(e.to_string()))?;
        m.get(&target.key()).map(|(_, st)| st.clone()).ok_or_else(|| SchedulerError::Unknown(target.key()))
    }
}

impl FakeScheduler {
    async fn with_state(
        &self,
        target: &FireTarget,
        f: impl FnOnce(&mut ScheduleState),
    ) -> Result<(), SchedulerError> {
        let mut m = self.inner.lock().map_err(|e| SchedulerError::Unavailable(e.to_string()))?;
        match m.get_mut(&target.key()) {
            Some((_, st)) => {
                f(st);
                Ok(())
            }
            None => Err(SchedulerError::Unknown(target.key())),
        }
    }
}
