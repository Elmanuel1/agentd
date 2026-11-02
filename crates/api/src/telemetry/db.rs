//! The SQL boundary: one span and one duration sample per statement, for
//! every statement any store runs, and a failed `CONNECT` sample per
//! connection that cannot be opened.

use std::fmt::Write;
use std::sync::Arc;
use std::time::Instant;

use diesel::connection::{Instrumentation, InstrumentationEvent};
use opentelemetry::KeyValue;
use store_postgres::{DieselPgPool, PgObserver};

use super::Metrics;

/// The pool's observer, recording into `metrics`.
pub fn observer(metrics: Arc<Metrics>) -> Arc<dyn PgObserver> {
    Arc::new(Pool { metrics })
}

/// Gauge the pool: connections open and in use, the cap, and callers waiting
/// for a connection (the sign the cap is the limit).
pub fn watch_pool(meter: &opentelemetry::metrics::Meter, pool: DieselPgPool) {
    let attrs = |state: &'static str| [KeyValue::new("db.client.connection.state", state)];
    let open = pool.clone();
    meter
        .i64_observable_gauge("db.client.connection.count")
        .with_callback(move |o| {
            let s = open.status();
            let idle = i64::try_from(s.available).unwrap_or(0);
            let size = i64::try_from(s.size).unwrap_or(0);
            o.observe(idle, &attrs("idle"));
            o.observe(size - idle, &attrs("used"));
        })
        .build();
    let cap = pool.clone();
    meter
        .i64_observable_gauge("db.client.connection.max")
        .with_callback(move |o| o.observe(i64::try_from(cap.status().max_size).unwrap_or(0), &[]))
        .build();
    meter
        .i64_observable_gauge("db.client.connection.pending_requests")
        .with_callback(move |o| o.observe(i64::try_from(pool.status().waiting).unwrap_or(0), &[]))
        .build();
}

struct Pool {
    metrics: Arc<Metrics>,
}

impl PgObserver for Pool {
    fn statements(&self) -> Box<dyn Instrumentation> {
        Box::new(Statements { metrics: self.metrics.clone(), running: None })
    }

    /// A connection that cannot be opened is a failed `CONNECT`, so the
    /// failed-statement series and alert see a database that is down.
    fn connect_failed(&self, error: &diesel::ConnectionError) {
        tracing::error!(error = %error, "postgres: could not open a connection");
        let attrs = [KeyValue::new("db.operation.name", "CONNECT"), KeyValue::new("outcome", "error")];
        self.metrics.db_duration.record(0.0, &attrs);
    }
}

struct Statements {
    metrics: Arc<Metrics>,
    running: Option<(Instant, &'static str, tracing::Span)>,
}

impl Instrumentation for Statements {
    fn on_connection_event(&mut self, event: InstrumentationEvent<'_>) {
        match event {
            InstrumentationEvent::StartQuery { query, .. } => {
                let op = operation(query);
                let span = tracing::info_span!(
                    "db.statement",
                    otel.name = %format!("{op} postgres"),
                    otel.kind = "client",
                    db.system.name = "postgresql",
                    db.operation.name = op,
                );
                self.running = Some((Instant::now(), op, span));
            }
            InstrumentationEvent::FinishQuery { error, .. } => {
                let Some((started, op, span)) = self.running.take() else {
                    return;
                };
                let outcome = if error.is_some() { "error" } else { "ok" };
                if let Some(e) = error {
                    span.in_scope(|| statement_failed(op, e));
                }
                let attrs = [KeyValue::new("db.operation.name", op), KeyValue::new("outcome", outcome)];
                self.metrics.db_duration.record(started.elapsed().as_secs_f64(), &attrs);
            }
            _ => {}
        }
    }
}

/// A unique violation is a conflict the caller sees as a 409; anything else
/// is agentd's failure.
fn statement_failed(op: &str, e: &diesel::result::Error) {
    use diesel::result::{DatabaseErrorKind, Error};
    match e {
        Error::DatabaseError(DatabaseErrorKind::UniqueViolation, _) | Error::NotFound => {
            tracing::debug!(db.operation.name = op, error = %e, "statement refused");
        }
        _ => tracing::error!(db.operation.name = op, error = %e, "statement failed"),
    }
}

/// The statement's leading keyword, read without formatting its binds (a
/// memory write binds its whole content).
fn operation(query: &dyn std::fmt::Display) -> &'static str {
    struct Head(String);
    impl Write for Head {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.push_str(s);
            if self.0.trim_start().contains(char::is_whitespace) || self.0.len() > 16 {
                return Err(std::fmt::Error);
            }
            Ok(())
        }
    }
    let mut head = Head(String::new());
    let _ = write!(head, "{query}");
    let word = head.0.split_whitespace().next().unwrap_or_default().to_ascii_uppercase();
    match word.as_str() {
        "SELECT" => "SELECT",
        "INSERT" => "INSERT",
        "UPDATE" => "UPDATE",
        "DELETE" => "DELETE",
        "WITH" => "WITH",
        "BEGIN" => "BEGIN",
        "COMMIT" => "COMMIT",
        "ROLLBACK" => "ROLLBACK",
        _ => "OTHER",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_statement_is_named_by_its_leading_keyword() {
        assert_eq!(super::operation(&"SELECT doc FROM memories"), "SELECT");
        assert_eq!(super::operation(&"  insert into x values ($1)"), "INSERT");
        assert_eq!(super::operation(&"WITH a AS (SELECT 1) SELECT * FROM a"), "WITH");
        assert_eq!(super::operation(&"VACUUM"), "OTHER");
        assert_eq!(super::operation(&format!("UPDATE t SET doc = '{}'", "x".repeat(1 << 20))), "UPDATE");
    }
}
