//! The webhook sender's signals: `agentd.webhooks.deliveries` by `outcome`
//! (`delivered`, `retried`, `dropped`), `agentd.webhooks.oldest_due_age` in
//! seconds, `agentd.webhooks.endpoints_disabled` by `reason`,
//! `agentd.webhooks.breaker_opened`, and `agentd.webhooks.claims` by
//! `instance`. Every replica reads the same table for the two gauges, so a
//! dashboard takes their maximum across replicas, not the sum.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Meter};
use webhooks::SenderMetrics;
use webhooks::states::DisabledReason;

/// The sender's instruments on one replica.
#[derive(Debug)]
pub struct WebhookMetrics {
    deliveries: Counter<u64>,
    breakers: Counter<u64>,
    claims: Counter<u64>,
    instance: KeyValue,
}

/// What the gauges last read.
#[derive(Debug, Default)]
struct Read {
    oldest_due: f64,
    disabled: HashMap<DisabledReason, usize>,
}

impl WebhookMetrics {
    /// Instruments on `meter`, labelling claims with `instance`.
    pub fn on(meter: &Meter, instance: &str) -> Arc<dyn SenderMetrics> {
        let read = Arc::new(Mutex::new(Read::default()));
        let (due, disabled) = (read.clone(), read.clone());
        meter
            .f64_observable_gauge("agentd.webhooks.oldest_due_age")
            .with_unit("s")
            .with_description("How long the longest-waiting due delivery has waited")
            .with_callback(move |o| {
                o.observe(due.lock().unwrap_or_else(PoisonError::into_inner).oldest_due, &[])
            })
            .build();
        meter
            .i64_observable_gauge("agentd.webhooks.endpoints_disabled")
            .with_description("Disabled endpoints, by reason")
            .with_callback(move |o| {
                let read = disabled.lock().unwrap_or_else(PoisonError::into_inner);
                for reason in
                    [DisabledReason::Redirect, DisabledReason::PrivateAddress, DisabledReason::Failing]
                {
                    let n = read.disabled.get(&reason).copied().unwrap_or(0);
                    o.observe(
                        i64::try_from(n).unwrap_or(i64::MAX),
                        &[KeyValue::new("reason", reason.as_str())],
                    );
                }
            })
            .build();
        Arc::new(Gauged {
            counters: Self {
                deliveries: meter
                    .u64_counter("agentd.webhooks.deliveries")
                    .with_description("Deliveries, by outcome")
                    .build(),
                breakers: meter
                    .u64_counter("agentd.webhooks.breaker_opened")
                    .with_description("An endpoint's breaker opened on this replica")
                    .build(),
                claims: meter
                    .u64_counter("agentd.webhooks.claims")
                    .with_description("Deliveries claimed, by instance")
                    .build(),
                instance: KeyValue::new("instance", instance.to_owned()),
            },
            read,
        })
    }
}

struct Gauged {
    counters: WebhookMetrics,
    read: Arc<Mutex<Read>>,
}

impl SenderMetrics for Gauged {
    fn delivery(&self, outcome: &'static str) {
        self.counters.deliveries.add(1, &[KeyValue::new("outcome", outcome)]);
    }

    fn breaker_opened(&self) {
        self.counters.breakers.add(1, &[]);
    }

    fn claimed(&self, n: usize) {
        self.counters
            .claims
            .add(u64::try_from(n).unwrap_or(u64::MAX), std::slice::from_ref(&self.counters.instance));
    }

    fn oldest_due(&self, age: Option<Duration>) {
        self.read.lock().unwrap_or_else(PoisonError::into_inner).oldest_due =
            age.unwrap_or_default().as_secs_f64();
    }

    fn disabled(&self, by_reason: &HashMap<DisabledReason, usize>) {
        self.read.lock().unwrap_or_else(PoisonError::into_inner).disabled.clone_from(by_reason);
    }
}
