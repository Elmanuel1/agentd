//! The edge's refusals: `agentd.http.rejected` by `reason` (`rate_limited`,
//! `timeout`), and `agentd.http.rate_limit_unchecked`, requests admitted with
//! Valkey slow or down.

use std::sync::OnceLock;

use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;

struct Instruments {
    rejected: Counter<u64>,
    unchecked: Counter<u64>,
}

fn instruments() -> &'static Instruments {
    static ONCE: OnceLock<Instruments> = OnceLock::new();
    ONCE.get_or_init(|| {
        let m = opentelemetry::global::meter("agentd");
        Instruments {
            rejected: m
                .u64_counter("agentd.http.rejected")
                .with_description("Requests refused, by reason")
                .build(),
            unchecked: m
                .u64_counter("agentd.http.rate_limit_unchecked")
                .with_description("Requests admitted without a rate check")
                .build(),
        }
    })
}

/// A request was refused for `reason`.
pub fn rejected(reason: &'static str) {
    instruments().rejected.add(1, &[KeyValue::new("reason", reason)]);
}

/// A request was admitted without its rate check.
pub fn rate_unchecked() {
    instruments().unchecked.add(1, &[]);
}
