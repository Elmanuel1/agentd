//! The edge's refusals and caches: `agentd.http.rejected` by `reason`
//! (`rate_limited`, `timeout`, `overloaded`), `agentd.http.rate_limit_unchecked`,
//! requests admitted with Valkey slow or down, and `agentd.keys.cache` by
//! `result` (`hit`, `miss`).

use std::sync::OnceLock;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Meter};

struct Instruments {
    rejected: Counter<u64>,
    unchecked: Counter<u64>,
    keys: Counter<u64>,
}

impl Instruments {
    fn on(m: &Meter) -> Self {
        Self {
            rejected: m
                .u64_counter("agentd.http.rejected")
                .with_description("Requests refused, by reason")
                .build(),
            unchecked: m
                .u64_counter("agentd.http.rate_limit_unchecked")
                .with_description("Requests admitted without a rate check")
                .build(),
            keys: m.u64_counter("agentd.keys.cache").with_description("API key lookups, by result").build(),
        }
    }

    fn rejected(&self, reason: &'static str) {
        self.rejected.add(1, &[KeyValue::new("reason", reason)]);
    }

    fn unchecked(&self) {
        self.unchecked.add(1, &[]);
    }

    fn key(&self, result: &'static str) {
        self.keys.add(1, &[KeyValue::new("result", result)]);
    }
}

fn instruments() -> &'static Instruments {
    static ONCE: OnceLock<Instruments> = OnceLock::new();
    ONCE.get_or_init(|| Instruments::on(&opentelemetry::global::meter("agentd")))
}

/// A request was refused for `reason`.
pub fn rejected(reason: &'static str) {
    instruments().rejected(reason);
}

/// A request was admitted without its rate check.
pub fn rate_unchecked() {
    instruments().unchecked();
}

/// A key lookup was answered from the cache (`hit`) or from Postgres (`miss`).
pub fn key_cache(result: &'static str) {
    instruments().key(result);
}

#[cfg(test)]
mod tests {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    use super::*;
    use crate::telemetry::webhooks::tests::points;

    #[test]
    fn each_refusal_counts_by_its_reason_and_an_unchecked_admission_counts_once() {
        let out = InMemoryMetricExporter::default();
        let meters =
            SdkMeterProvider::builder().with_reader(PeriodicReader::builder(out.clone()).build()).build();
        let i = Instruments::on(&meters.meter("test"));
        for reason in ["rate_limited", "rate_limited", "timeout", "overloaded"] {
            i.rejected(reason);
        }
        i.unchecked();
        meters.force_flush().unwrap();
        let owned = |v: &[(&str, &str)]| {
            v.iter().map(|(k, n)| ((*k).to_owned(), (*n).to_owned())).collect::<Vec<_>>()
        };
        assert_eq!(
            points(&out, "agentd.http.rejected"),
            owned(&[("overloaded", "1"), ("rate_limited", "2"), ("timeout", "1")])
        );
        assert_eq!(points(&out, "agentd.http.rate_limit_unchecked"), owned(&[("", "1")]));
    }

    #[test]
    fn each_key_lookup_counts_by_whether_the_cache_answered() {
        // SH-44: `agentd.keys.cache` by `result`.
        let out = InMemoryMetricExporter::default();
        let meters =
            SdkMeterProvider::builder().with_reader(PeriodicReader::builder(out.clone()).build()).build();
        let i = Instruments::on(&meters.meter("test"));
        for result in ["hit", "hit", "miss"] {
            i.key(result);
        }
        meters.force_flush().unwrap();
        let counted: Vec<(String, String)> = vec![("hit".into(), "2".into()), ("miss".into(), "1".into())];
        assert_eq!(points(&out, "agentd.keys.cache"), counted);
    }
}
