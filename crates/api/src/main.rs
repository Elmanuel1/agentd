//! agentd — the Managed-Agents-compatible control plane.
//!
//! Boot: read [`Config`] from the environment, connect the backends it
//! selects, assemble the app, and serve. `agentd keys …`, `agentd
//! admin-keys …` and `agentd environment-keys …` manage keys, `agentd
//! vault-secrets tag|reseal` maintains vault secrets, and `agentd webhooks
//! enable` re-enables a disabled endpoint, instead ([`api::cli`]). Records and blobs live
//! in Postgres and object storage. Every replica's event streams read the log,
//! signalled through Valkey (`AGENTD_VALKEY_URL`), and every replica claims
//! due schedules from Postgres.

use std::process::ExitCode;

use api::{AppState, Backends, Config, build_router};

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
        Some((cmd, rest)) if cmd == "admin-keys" => {
            return api::cli::admin_keys::admin_keys(&config, rest).await;
        }
        Some((cmd, rest)) if cmd == "environment-keys" => {
            return api::cli::environment_keys::environment_keys(&config, rest).await;
        }
        Some((cmd, rest)) if cmd == "vault-secrets" => {
            return api::cli::vault_secrets::vault_secrets(&config, rest).await;
        }
        Some((cmd, rest)) if cmd == "webhooks" => return api::cli::webhooks::webhooks(&config, rest).await,
        Some(_) => {
            let cli = api::cli::USAGE;
            let (env, vault) = (api::cli::environment_keys::USAGE, api::cli::vault_secrets::USAGE);
            eprintln!("{cli}\n{env}\n{vault}\n{}", api::cli::webhooks::USAGE);
            return ExitCode::FAILURE;
        }
    }
    let _telemetry = match api::telemetry::init(&config.telemetry) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("telemetry: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(url) = &config.model_prices_url {
        drop(engine::model::price_feed::spawn(url.clone()));
    }
    if config.cloud_swap_misses_secrets() {
        tracing::warn!(
            "AGENTD_SECRETS is postgres and AGENTD_RUNTIME_KEYS is set: the cloud egress swap reads only aws_sm \
             or aws_ssm, so environment_variable credentials in cloud sessions are not swapped"
        );
    }
    let mut backends = match Backends::from_config(&config).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "backend connect failed");
            return ExitCode::FAILURE;
        }
    };
    let shutdown = std::mem::take(&mut backends.shutdown);
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
    let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
        shutdown_signal().await;
        signal.notify_one();
    });
    let mut server = tokio::spawn(serve.into_future());
    tokio::select! {
        served = &mut server => return ended(served),
        () = stopping.notified() => {}
    }
    // HTTP, the loop and the webhook sender drain at once, inside one grace (SH-42). An open event
    // stream never ends on its own, so past the grace it is dropped and the telemetry still flushes.
    let deadline = tokio::time::Instant::now() + config.agent_loop.grace;
    let (served, ()) = tokio::join!(tokio::time::timeout_at(deadline, &mut server), shutdown.run(deadline));
    match served {
        Ok(served) => ended(served),
        Err(_) => {
            server.abort();
            tracing::warn!(ms = config.agent_loop.grace.as_millis(), "drain timed out; closing open streams");
            ExitCode::SUCCESS
        }
    }
}

fn ended(served: Result<std::io::Result<()>, tokio::task::JoinError>) -> ExitCode {
    match served {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            tracing::error!(error = %e, "server error");
            ExitCode::FAILURE
        }
        Err(e) => {
            tracing::error!(error = %e, "the server task failed");
            ExitCode::FAILURE
        }
    }
}

/// SIGTERM or Ctrl-C: stop accepting, finish in-flight requests, then return
/// so the telemetry guard flushes the last spans, logs and metrics.
async fn shutdown_signal() {
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
