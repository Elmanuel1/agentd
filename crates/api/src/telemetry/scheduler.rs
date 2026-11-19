//! The scheduler's signals: `agentd.scheduler.fires`, the fires by `outcome`
//! (`enqueued`, `ran`, `retried`, `skipped`, `dropped`);
//! `agentd.scheduler.fire.lag`, a run's start minus its occurrence's due time,
//! in seconds; and `agentd.scheduler.fires.due`, the due fires not done,
//! counted every 30 s on every replica. Every replica counts the same table,
//! so a dashboard takes the maximum of the gauge across replicas, not the sum.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};
use shared::ports::fire::Ending;
use store_postgres::PgFireClaims;

/// How often the due fires are counted.
const EVERY: Duration = Duration::from_secs(30);
/// Lag buckets in seconds: on time is under the 1 s worker interval; a fire
/// still waiting after minutes is behind.
const LAG: [f64; 11] = [0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0];

/// The fire counter and the lag histogram.
#[derive(Debug, Clone)]
pub struct SchedulerMetrics {
    fires: Counter<u64>,
    lag: Histogram<f64>,
}

impl SchedulerMetrics {
    /// Instruments on `meter`.
    pub fn new(meter: &Meter) -> Self {
        Self {
            fires: meter.u64_counter("agentd.scheduler.fires").with_description("Fires by outcome").build(),
            lag: meter
                .f64_histogram("agentd.scheduler.fire.lag")
                .with_unit("s")
                .with_boundaries(LAG.to_vec())
                .with_description("A fire's start minus its due time")
                .build(),
        }
    }

    /// The tick wrote `n` occurrences to the fire queue.
    pub fn enqueued(&self, n: usize) {
        self.fires.add(u64::try_from(n).unwrap_or(u64::MAX), &[KeyValue::new("outcome", "enqueued")]);
    }

    /// A worker settled a fire as `ending`, which started `lag` after its due time when it ran.
    pub fn ended(&self, ending: Ending, lag: Option<Duration>) {
        let outcome = match ending {
            Ending::Done(outcome) => outcome.as_str(),
            Ending::Retried => "retried",
        };
        self.fires.add(1, &[KeyValue::new("outcome", outcome)]);
        if let Some(lag) = lag {
            self.lag.record(lag.as_secs_f64(), &[]);
        }
    }
}

/// Count the due fires of `claims` every 30 s into `agentd.scheduler.fires.due`, until the task is aborted.
pub fn watch_due(meter: &Meter, claims: PgFireClaims) -> tokio::task::JoinHandle<()> {
    let due = Arc::new(AtomicI64::new(0));
    let read = due.clone();
    meter
        .i64_observable_gauge("agentd.scheduler.fires.due")
        .with_description("Fires due and not done")
        .with_callback(move |o| o.observe(read.load(Ordering::Relaxed), &[]))
        .build();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(EVERY);
        loop {
            every.tick().await;
            match claims.count_due().await {
                Ok(n) => due.store(n, Ordering::Relaxed),
                Err(e) => tracing::warn!(error = %e, "scheduler: could not count the due fires"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
    use shared::ports::fire::Outcome;

    use super::*;

    #[test]
    fn each_fire_counts_by_its_outcome_and_a_run_records_its_lag() {
        let out = InMemoryMetricExporter::default();
        let meters =
            SdkMeterProvider::builder().with_reader(PeriodicReader::builder(out.clone()).build()).build();
        let metrics = SchedulerMetrics::new(&meters.meter("test"));
        metrics.enqueued(3);
        metrics.ended(Ending::Done(Outcome::Ran), Some(Duration::from_secs(2)));
        metrics.ended(Ending::Retried, Some(Duration::from_millis(300)));
        metrics.ended(Ending::Done(Outcome::Skipped), None);
        metrics.ended(Ending::Done(Outcome::Dropped), None);
        meters.force_flush().unwrap();

        let finished = out.get_finished_metrics().unwrap();
        let all: Vec<_> =
            finished.iter().flat_map(|rm| rm.scope_metrics()).flat_map(|s| s.metrics()).collect();
        let mut counts: Vec<(String, u64)> = all
            .iter()
            .filter(|m| m.name() == "agentd.scheduler.fires")
            .flat_map(|m| match m.data() {
                AggregatedMetrics::U64(MetricData::Sum(sum)) => sum
                    .data_points()
                    .map(|p| (p.attributes().map(|kv| kv.value.to_string()).collect::<String>(), p.value()))
                    .collect(),
                _ => vec![],
            })
            .collect();
        counts.sort();
        let want = [("dropped", 1), ("enqueued", 3), ("ran", 1), ("retried", 1), ("skipped", 1)];
        assert_eq!(counts, want.map(|(o, n)| (o.to_owned(), n)));
        let lags: u64 = all
            .iter()
            .filter(|m| m.name() == "agentd.scheduler.fire.lag")
            .flat_map(|m| match m.data() {
                AggregatedMetrics::F64(MetricData::Histogram(h)) => {
                    h.data_points().map(|p| p.count()).collect()
                }
                _ => vec![],
            })
            .sum();
        assert_eq!(lags, 2, "only fires that started record a lag");
    }
}
