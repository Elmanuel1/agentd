//! agentd — the Managed-Agents-compatible control plane.
//!
//! Boot: build the in-memory stack (persistent stores slot in behind the
//! same traits), register API keys from `AGENTD_API_KEYS`
//! (`key:org:workspace[,key:org:workspace…]`), start the local scheduler,
//! and serve.

use std::sync::Arc;

use agentd_api::{build_router, AppState};
use agentd_core::ports::FireTarget;
use agentd_scheduler::{FireError, FireSink, LocalScheduler};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// In-process fire sink: scheduled ticks call the deployments service
/// directly (same fire contract as the HTTP `/internal/fire`).
struct InProcessSink {
    state: parking_lot::Mutex<Option<AppState>>,
}

#[async_trait]
impl FireSink for InProcessSink {
    async fn fire(
        &self,
        target: &FireTarget,
        scheduled_at: DateTime<Utc>,
    ) -> Result<(), FireError> {
        let Some(state) = self.state.lock().clone() else {
            return Err(FireError("state not initialized".into()));
        };
        // The scheduler fires under the service identity; the deployment's
        // tenant travels in the schedule spec → resolved by the registry.
        // (In-process: fire under every registered tenant is wrong — the
        // deployments service is workspace-scoped, so use the spec tenant
        // when LocalScheduler grows tenant plumbing; for now dev mode uses
        // the default tenant.)
        let Some(ctx) = state.keys.resolve("agentd-dev-key") else {
            return Err(FireError("no dev tenant".into()));
        };
        match target {
            FireTarget::Deployment { id } => {
                state
                    .deployments
                    .fire_scheduled(
                        &ctx,
                        id,
                        agentd_core::Timestamp::from_datetime(scheduled_at),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|e| FireError(e.to_string()))
            }
            FireTarget::Dream { .. } => Ok(()), // dreams service wiring: next phase
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber_init();
    let sink = Arc::new(InProcessSink {
        state: parking_lot::Mutex::new(None),
    });
    let scheduler = Arc::new(LocalScheduler::new(sink.clone()));
    let state = AppState::in_memory(scheduler.clone());

    // Keys from env: AGENTD_API_KEYS="key:org:workspace,key2:org2:ws2"
    let keys_env = std::env::var("AGENTD_API_KEYS")
        .unwrap_or_else(|_| "agentd-dev-key:org_dev:wrkspc_01dev".into());
    for (i, triple) in keys_env.split(',').enumerate() {
        let mut parts = triple.splitn(3, ':');
        if let (Some(key), Some(org), Some(ws)) = (parts.next(), parts.next(), parts.next()) {
            state
                .keys
                .register(key, org, ws, format!("apikey_01local{i:010}"));
        }
    }
    *sink.state.lock() = Some(state.clone());

    // Background cron ticker (10s resolution; cron is minute-granular).
    let _ticker = scheduler.spawn(std::time::Duration::from_secs(10));

    let addr = std::env::var("AGENTD_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let app = build_router(state);
    tracing::info!(%addr, "agentd listening");
    match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!(error = %e, "server error");
            }
        }
        Err(e) => tracing::error!(error = %e, %addr, "bind failed"),
    }
}

fn tracing_subscriber_init() {
    // Minimal env-filter logging without extra deps.
    let level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into());
    let _ = tracing::subscriber::set_global_default(
        tracing_subscriber_stub::Subscriber::new(level),
    );
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
            Self {
                level,
                ids: AtomicU64::new(1),
            }
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
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    use std::fmt::Write;
                    let _ = write!(self.0, " {}={value:?}", field.name());
                }
            }
            let mut v = V(String::new());
            event.record(&mut v);
            println!(
                "[{}] {}{}",
                event.metadata().level(),
                event.metadata().target(),
                v.0
            );
        }
        fn enter(&self, _span: &span::Id) {}
        fn exit(&self, _span: &span::Id) {}
    }
}
