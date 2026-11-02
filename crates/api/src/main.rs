//! agentd — the Managed-Agents-compatible control plane.
//!
//! Boot: read [`Config`] from the environment, connect the backends it
//! selects, assemble the app, and serve. Records and blobs live in Postgres and
//! object storage; event streams and the local scheduler live in the process,
//! so run one replica until a shared event bus and scheduler replace them.

use std::process::ExitCode;

use api::{build_router, AppState, Backends, Config};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber_init();
    let config = match Config::from_lookup(|var| std::env::var(var).ok()) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "invalid configuration");
            return ExitCode::FAILURE;
        }
    };
    let backends = match Backends::from_config(&config).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "backend connect failed");
            return ExitCode::FAILURE;
        }
    };
    let app = match AppState::build(backends) {
        Ok(state) => build_router(state),
        Err(e) => {
            tracing::error!(error = %e, "module assembly failed");
            return ExitCode::FAILURE;
        }
    };
    let listener = match tokio::net::TcpListener::bind(&config.addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, addr = %config.addr, "bind failed");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(addr = %config.addr, "agentd listening");
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "server error");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn tracing_subscriber_init() {
    // Minimal env-filter logging without extra deps.
    let level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into());
    let _ = tracing::subscriber::set_global_default(tracing_subscriber_stub::Subscriber::new(level));
}

/// Tiny stdout subscriber (avoids pulling tracing-subscriber; swap freely).
mod tracing_subscriber_stub {
    use std::sync::atomic::{AtomicU64, Ordering};
    use tracing::span;

    /// Minimal subscriber printing events to stdout.
    pub struct Subscriber {
        level: tracing::Level,
        ids: AtomicU64,
    }

    impl Subscriber {
        /// Build with a level string.
        pub fn new(level: String) -> Self {
            let level = match level.to_ascii_lowercase().as_str() {
                "trace" => tracing::Level::TRACE,
                "debug" => tracing::Level::DEBUG,
                "warn" => tracing::Level::WARN,
                "error" => tracing::Level::ERROR,
                _ => tracing::Level::INFO,
            };
            Self { level, ids: AtomicU64::new(1) }
        }
    }

    impl tracing::Subscriber for Subscriber {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            *metadata.level() <= self.level
        }
        fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(self.ids.fetch_add(1, Ordering::Relaxed))
        }
        fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}
        fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            struct V(String);
            impl tracing::field::Visit for V {
                fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                    use std::fmt::Write;
                    let _ = write!(self.0, " {}={value:?}", field.name());
                }
            }
            let mut v = V(String::new());
            event.record(&mut v);
            println!("[{}] {}{}", event.metadata().level(), event.metadata().target(), v.0);
        }
        fn enter(&self, _span: &span::Id) {}
        fn exit(&self, _span: &span::Id) {}
    }
}
