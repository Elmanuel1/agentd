//! agentd — the Managed-Agents-compatible control plane.
//!
//! Boot: read [`Config`] from the environment, connect the backends it
//! selects, assemble the app, and serve. Replicas hold no state of their own.

use std::process::ExitCode;

use api::{build_router, AppState, Backends, Config};

#[tokio::main]
async fn main() -> ExitCode {
    // No subscriber exists before the config is read, so a bad config goes
    // to stderr.
    let config = match Config::from_lookup(|var| std::env::var(var).ok()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("invalid configuration: {e}");
            return ExitCode::FAILURE;
        }
    };
    let _telemetry = match api::telemetry::init(config.otlp, config.json_logs) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("telemetry: {e}");
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
    if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown()).await {
        tracing::error!(error = %e, "server error");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// SIGTERM or Ctrl-C: stop accepting, finish in-flight requests, then return
/// so the telemetry guard flushes the last spans, logs and metrics.
async fn shutdown() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("agentd shutting down");
}
