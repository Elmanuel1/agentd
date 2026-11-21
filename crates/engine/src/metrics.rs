//! The loop's signals, exported over OTLP with the rest of agentd's metrics
//! (`docs/operations.md` names each with its threshold): how long a due step
//! waits for a worker, how long model calls take, how turns end, the events
//! steps read and append, compactions, and MCP calls.

use std::sync::OnceLock;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};
use sessions::events::{Event, EventPayload, SessionEvent, StopReason};

struct Instruments {
    step_wait: Histogram<f64>,
    model_call: Histogram<f64>,
    turns: Counter<u64>,
    events_read: Histogram<u64>,
    events_appended: Counter<u64>,
    compactions: Counter<u64>,
    mcp_calls: Counter<u64>,
    released: Counter<u64>,
    step_failures: Counter<u64>,
    breaker_opened: Counter<u64>,
}

fn instruments() -> &'static Instruments {
    static ONCE: OnceLock<Instruments> = OnceLock::new();
    ONCE.get_or_init(|| Instruments::on(&opentelemetry::global::meter("agentd")))
}

impl Instruments {
    fn on(m: &Meter) -> Self {
        Self {
            step_wait: m
                .f64_histogram("agentd.loop.step_wait")
                .with_unit("s")
                .with_description("Time a due step waited before a worker claimed it")
                .build(),
            model_call: m
                .f64_histogram("agentd.loop.model_call_duration")
                .with_unit("s")
                .with_description("One model call, its retries included")
                .build(),
            turns: m.u64_counter("agentd.loop.turns").with_description("Turns ended, by stop reason").build(),
            events_read: m
                .u64_histogram("agentd.loop.events_read")
                .with_description("Events one step read from its session's log")
                .build(),
            events_appended: m
                .u64_counter("agentd.loop.events_appended")
                .with_description("Events steps appended")
                .build(),
            compactions: m
                .u64_counter("agentd.loop.compactions")
                .with_description("Compaction calls, by outcome")
                .build(),
            mcp_calls: m
                .u64_counter("agentd.loop.mcp_calls")
                .with_description("MCP tool calls, by outcome")
                .build(),
            released: m
                .u64_counter("agentd.loop.released")
                .with_description("Steps released unfinished when the shutdown grace ended")
                .build(),
            step_failures: m
                .u64_counter("agentd.loop.step_failures")
                .with_description("Steps that failed and wait to run again")
                .build(),
            breaker_opened: m
                .u64_counter("agentd.model.breaker_opened")
                .with_description("A provider's breaker opened on this replica, by provider")
                .build(),
        }
    }

    fn release(&self, n: u64) {
        self.released.add(n, &[]);
    }

    fn step_failed(&self) {
        self.step_failures.add(1, &[]);
    }

    fn breaker_opened(&self, provider: &'static str) {
        self.breaker_opened.add(1, &[KeyValue::new("provider", provider)]);
    }
}

/// A claim that waited `ms` past its due time.
pub(crate) fn claimed(ms: i64) {
    instruments().step_wait.record(f64::from(u32::try_from(ms.max(0)).unwrap_or(u32::MAX)) / 1000.0, &[]);
}

/// A model call to `provider` that took `seconds` and ended `outcome`.
pub(crate) fn model_call(provider: &'static str, seconds: f64, outcome: &'static str) {
    let attrs = [KeyValue::new("provider", provider), KeyValue::new("outcome", outcome)];
    instruments().model_call.record(seconds, &attrs);
}

/// A step read `n` events.
pub(crate) fn read(n: usize) {
    instruments().events_read.record(u64::try_from(n).unwrap_or(u64::MAX), &[]);
}

/// A step appended `events`; each idle among them ended a turn.
pub(crate) fn appended(events: &[Event]) {
    let i = instruments();
    i.events_appended.add(u64::try_from(events.len()).unwrap_or(u64::MAX), &[]);
    for e in events {
        if let EventPayload::Session(SessionEvent::StatusIdle { stop_reason }) = &e.payload {
            let why = match stop_reason {
                StopReason::EndTurn {} => "end_turn",
                StopReason::RequiresAction { .. } => "requires_action",
                StopReason::RetriesExhausted {} => "retries_exhausted",
                StopReason::BudgetReached {} => "budget_reached",
            };
            i.turns.add(1, &[KeyValue::new("stop_reason", why)]);
        }
    }
}

/// A compaction call that ended `outcome` (`compacted`, `no_summary`, `failed`).
pub(crate) fn compaction(outcome: &'static str) {
    instruments().compactions.add(1, &[KeyValue::new("outcome", outcome)]);
}

/// An MCP tool call that ended `outcome` (`ok`, `tool_error`, `unreachable`).
pub(crate) fn mcp_call(outcome: &'static str) {
    instruments().mcp_calls.add(1, &[KeyValue::new("outcome", outcome)]);
}

/// The grace ended with `n` steps running; they were released.
pub(crate) fn released(n: u64) {
    instruments().release(n);
}

/// A step failed; it runs again after its backoff.
pub(crate) fn step_failed() {
    instruments().step_failed();
}

/// `provider`'s breaker opened.
pub(crate) fn model_breaker_opened(provider: &'static str) {
    instruments().breaker_opened(provider);
}

#[cfg(test)]
mod tests {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    use super::*;

    #[test]
    fn the_shutdown_failure_and_breaker_counters_are_exported_with_their_labels() {
        let out = InMemoryMetricExporter::default();
        let meters =
            SdkMeterProvider::builder().with_reader(PeriodicReader::builder(out.clone()).build()).build();
        let i = Instruments::on(&meters.meter("test"));
        i.release(3);
        i.step_failed();
        i.breaker_opened("anthropic");
        meters.force_flush().unwrap();
        let finished = out.get_finished_metrics().unwrap();
        let mut points: Vec<(String, String, u64)> = finished
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .flat_map(|s| s.metrics())
            .flat_map(|m| match m.data() {
                AggregatedMetrics::U64(MetricData::Sum(sum)) => sum
                    .data_points()
                    .map(|p| {
                        let labels = p.attributes().map(|kv| kv.value.to_string()).collect::<String>();
                        (m.name().to_owned(), labels, p.value())
                    })
                    .collect(),
                _ => vec![],
            })
            .collect();
        points.sort();
        let want = [
            ("agentd.loop.released", "", 3),
            ("agentd.loop.step_failures", "", 1),
            ("agentd.model.breaker_opened", "anthropic", 1),
        ];
        assert_eq!(points, want.map(|(n, l, v)| (n.to_owned(), l.to_owned(), v)));
    }
}
