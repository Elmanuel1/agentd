//! agentd-egress: hardens the process, then serves the proxy and the admin
//! interface.

use std::process::ExitCode;
use std::sync::Arc;

use egress_proxy::{admin, serve, Registry};
use secrets_aws::AwsSecretReader;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use egress::config::Config;
use egress::harden;
use egress::source::VaultSource;

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(e) = harden::process() {
        eprintln!("agentd-egress: {e}");
        return ExitCode::FAILURE;
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "agentd-egress stopped");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let config = Config::from_lookup(&|k| std::env::var(k).ok())?;
    let reader = AwsSecretReader::connect(
        &config.prefix,
        config.region.as_deref(),
        config.endpoint.as_deref(),
        config.role_arn.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;
    let registry = Arc::new(Registry::new(config.internal_hosts.clone()));
    let proxy = TcpListener::bind(&config.addr).await.map_err(|e| format!("{}: {e}", config.addr))?;
    let admin_listener =
        TcpListener::bind(&config.admin_addr).await.map_err(|e| format!("{}: {e}", config.admin_addr))?;
    tracing::info!(proxy = %config.addr, admin = %config.admin_addr, "agentd-egress listening");
    let router = admin::router(registry.clone(), &config.admin_token);
    tokio::select! {
        () = serve(proxy, registry, Arc::new(VaultSource(reader))) => Ok(()),
        served = axum::serve(admin_listener, router) => served.map_err(|e| e.to_string()),
    }
}
