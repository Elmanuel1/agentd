//! agentd-worker: the process a session's sandbox runs. It acknowledges the
//! session's work item and keeps its lease with heartbeats, as the SDK's
//! environment worker does. It opens no port: every call goes out to agentd,
//! with the work token from the work's secret.
//!
//! Environment: `AGENTD_API_URL`, `AGENTD_ENVIRONMENT_ID`, `AGENTD_WORK_ID`,
//! `AGENTD_WORK_SECRET`.
//!
//! `agentd-worker relay` runs the gateway instead (see `relay`).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

mod client;
mod relay;

use client::{Client, Config};

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("relay") {
        match relay::parse(&std::env::var("AGENTD_RELAY").unwrap_or_default()) {
            Ok(pairs) => relay::run(pairs).await,
            Err(e) => eprintln!("agentd-worker: {e}"),
        }
        std::process::exit(1);
    }
    let config = match Config::from_env(|k| std::env::var(k).ok()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("agentd-worker: {e}");
            std::process::exit(2);
        }
    };
    let client = Client::new(config);
    client.ack_until_done().await;
    // The lease ends the worker. Work agentd is stopping is stopped; a lost
    // lease is left alone, since another holder may own the work now.
    match client.keep_lease().await {
        client::LeaseEnd::Stopping => client.stop().await,
        client::LeaseEnd::Lost(reason) => eprintln!("agentd-worker: stopping: {reason}"),
    }
}
