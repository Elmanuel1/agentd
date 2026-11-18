//! The change feed's slot and resource history, the signals that history has
//! a gap or will have one:
//!
//! - `agentd.feed.slot.retained_wal` (bytes): the WAL the slot holds back,
//! - `agentd.feed.slot.safe_wal` (bytes): what may still be written before
//!   Postgres invalidates the slot (absent while `max_slot_wal_keep_size` is
//!   unlimited),
//! - `agentd.feed.slot.lost`: 1 when the slot is invalidated or missing,
//! - `agentd.feed.wal_written` (bytes): the WAL Postgres has written, for
//!   sizing,
//!
//! read every 10 s from the primary with one statement ([`read_slot`]), on every replica (so
//! take the maximum across replicas, not the sum); and
//! `agentd.history.apply_delay` (s), how long after its commit each change
//! reached history.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use store_postgres::{DieselPgPool, Slot, read_slot};

/// How often the slot is read.
const EVERY: Duration = Duration::from_secs(10);

/// A gauge with nothing to report, such as a missing slot's retained WAL.
const ABSENT: i64 = i64::MIN;

/// The apply delay's buckets in seconds, up to an hour: a stalled feed shows
/// as minutes, not as the HTTP buckets' 10 s.
const DELAY: [f64; 12] = [0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0];

/// The slot's last reading, each value [`ABSENT`] when it has nothing to report.
#[derive(Clone)]
struct Readings([Arc<AtomicI64>; 4]);

impl Readings {
    fn new() -> Self {
        Self([(); 4].map(|()| Arc::new(AtomicI64::new(ABSENT))))
    }

    /// Store a read of the slot; a failed read (`None`) leaves every value
    /// absent, so a stale reading never reports as current.
    fn set(&self, slot: Option<&Slot>) {
        let [retained, safe, lost, written] = &self.0;
        let value = |f: fn(&Slot) -> Option<i64>| slot.and_then(f).unwrap_or(ABSENT);
        retained.store(value(|s| s.retained), Ordering::Relaxed);
        safe.store(value(|s| s.safe), Ordering::Relaxed);
        lost.store(value(|s| Some(i64::from(s.lost))), Ordering::Relaxed);
        written.store(value(|s| Some(s.wal_written)), Ordering::Relaxed);
    }
}

/// Read `slot` every 10 s into the gauges, until the task is aborted. A
/// gauge with nothing to report (a missing slot's retained WAL, or any value
/// while the slot cannot be read) is absent.
pub fn watch(
    meter: &opentelemetry::metrics::Meter,
    pool: DieselPgPool,
    slot: &str,
) -> tokio::task::JoinHandle<()> {
    let readings = Readings::new();
    let [retained, safe, lost, written] = &readings.0;
    let gauge = |name: &'static str, unit: &'static str, about: &'static str, value: &Arc<AtomicI64>| {
        let value = value.clone();
        let builder = meter.i64_observable_gauge(name).with_description(about).with_unit(unit);
        builder
            .with_callback(move |o| {
                let v = value.load(Ordering::Relaxed);
                if v != ABSENT {
                    o.observe(v, &[]);
                }
            })
            .build();
    };
    gauge("agentd.feed.slot.retained_wal", "By", "WAL the change feed's slot holds back", retained);
    gauge("agentd.feed.slot.safe_wal", "By", "WAL that may be written before the slot is lost", safe);
    gauge("agentd.feed.slot.lost", "1", "1 when the change feed's slot is lost or missing", lost);
    let read_written = written.clone();
    meter
        .u64_observable_counter("agentd.feed.wal_written")
        .with_unit("By")
        .with_description("WAL Postgres has written")
        .with_callback(move |o| {
            if let Ok(v) = u64::try_from(read_written.load(Ordering::Relaxed)) {
                o.observe(v, &[]);
            }
        })
        .build();
    let slot = slot.to_owned();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(EVERY);
        loop {
            every.tick().await;
            match read_slot(&pool, &slot).await {
                Ok(s) => readings.set(Some(&s)),
                Err(e) => {
                    readings.set(None);
                    tracing::warn!(%slot, error = %e, "feed: could not read the slot");
                }
            }
        }
    })
}

/// The history consumer's apply delay, recorded into `agentd.history.apply_delay`.
pub fn apply_delay(meter: &opentelemetry::metrics::Meter) -> history::Applied {
    let delay = meter
        .f64_histogram("agentd.history.apply_delay")
        .with_unit("s")
        .with_boundaries(DELAY.to_vec())
        .with_description("How long after its commit each change reached history")
        .build();
    Arc::new(move |late| delay.record(late.as_secs_f64(), &[]))
}

#[cfg(test)]
mod tests {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    #[test]
    fn a_failed_read_leaves_every_slot_value_absent() {
        let readings = super::Readings::new();
        let slot = store_postgres::Slot { retained: Some(10), safe: None, lost: false, wal_written: 99 };
        let values =
            |r: &super::Readings| r.0.each_ref().map(|v| v.load(std::sync::atomic::Ordering::Relaxed));
        readings.set(Some(&slot));
        assert_eq!(values(&readings), [10, super::ABSENT, 0, 99]);
        readings.set(None);
        assert_eq!(values(&readings), [super::ABSENT; 4]);
    }

    #[test]
    fn each_recorded_change_s_delay_lands_in_minute_scale_buckets() {
        let out = InMemoryMetricExporter::default();
        let meters =
            SdkMeterProvider::builder().with_reader(PeriodicReader::builder(out.clone()).build()).build();
        let applied = super::apply_delay(&meters.meter("test"));
        applied(std::time::Duration::from_secs(90));
        applied(std::time::Duration::from_millis(200));
        meters.force_flush().unwrap();
        let metrics = out.get_finished_metrics().unwrap();
        let point = metrics
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .flat_map(|s| s.metrics())
            .find(|m| m.name() == "agentd.history.apply_delay")
            .and_then(|m| match m.data() {
                AggregatedMetrics::F64(MetricData::Histogram(h)) => {
                    h.data_points().next().map(|p| (p.count(), p.bounds().collect::<Vec<_>>()))
                }
                _ => None,
            });
        assert_eq!(point, Some((2, super::DELAY.to_vec())));
    }
}
