//! `agentd.loop.steps_due` and `agentd.loop.steps_leased`: the step queue's
//! due and leased steps, counted every 10 s on every replica, which the loop
//! workers' autoscaler follows. Every replica reads the same table, so a
//! dashboard takes the maximum across replicas, not the sum.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sessions::steps::Steps;

/// How often the queue is counted.
const EVERY: Duration = Duration::from_secs(10);

/// Count `steps` every 10 s into the two gauges, until the task is aborted.
pub fn watch(meter: &opentelemetry::metrics::Meter, steps: Arc<dyn Steps>) -> tokio::task::JoinHandle<()> {
    let (due, leased) = (Arc::new(AtomicI64::new(0)), Arc::new(AtomicI64::new(0)));
    let (read_due, read_leased) = (due.clone(), leased.clone());
    meter
        .i64_observable_gauge("agentd.loop.steps_due")
        .with_description("Steps due and held by no live lease")
        .with_callback(move |o| o.observe(read_due.load(Ordering::Relaxed), &[]))
        .build();
    meter
        .i64_observable_gauge("agentd.loop.steps_leased")
        .with_description("Steps held under a live lease")
        .with_callback(move |o| o.observe(read_leased.load(Ordering::Relaxed), &[]))
        .build();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(EVERY);
        loop {
            every.tick().await;
            match steps.counts().await {
                Ok(c) => {
                    due.store(c.due, Ordering::Relaxed);
                    leased.store(c.leased, Ordering::Relaxed);
                }
                Err(e) => tracing::warn!(error = %e, "loop: could not count the step queue"),
            }
        }
    })
}
