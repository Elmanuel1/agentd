//! agentd — the Managed-Agents-compatible control plane.
//!
//! Boot: read [`Config`] from the environment, connect the backends it
//! selects, assemble the app, and serve. `agentd keys …` manages API keys
//! instead ([`api::cli`]). Records and blobs live in Postgres and
//! object storage; event streams and the local scheduler live in the process,
//! so run one replica until a shared event bus and scheduler replace them.

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        None => {}
        Some((cmd, rest)) if cmd == "keys" => return api::cli::keys(&config, rest).await,
        Some((cmd, rest)) if cmd == "model-key" => return api::model_key::model_key(&config, rest).await,
        Some(_) => {
            eprintln!("{}\n{}", api::cli::USAGE, api::model_key::USAGE);
            return ExitCode::FAILURE;
        }
    }
    let _telemetry = match api::telemetry::init(config.telemetry.otlp, config.telemetry.json_logs) {
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
    let stopping = std::sync::Arc::new(tokio::sync::Notify::new());
    let signal = stopping.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        shutdown().await;
        signal.notify_one();
    });
    // An open event stream never ends on its own, so draining is capped;
    // past it the streams are dropped and the telemetry still flushes.
    let drained = async {
        stopping.notified().await;
        tokio::time::sleep(DRAIN).await;
        tracing::warn!(seconds = DRAIN.as_secs(), "drain timed out; closing open streams");
    };
    tokio::select! {
        served = server => {
            if let Err(e) = served {
                tracing::error!(error = %e, "server error");
                return ExitCode::FAILURE;
            }
        }
        () = drained => {}
    }
    ExitCode::SUCCESS
}

/// How long in-flight requests get to finish after a shutdown signal.
const DRAIN: std::time::Duration = std::time::Duration::from_secs(10);

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
