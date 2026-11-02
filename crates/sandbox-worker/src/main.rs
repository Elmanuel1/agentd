//! agentd-worker: the process a session's sandbox runs. It acknowledges the
//! session's work item, keeps its lease with heartbeats (as the SDK's
//! environment worker does), and long-polls agentd for sandbox operations,
//! running each and posting its outcome. It opens no port: every call goes
//! out to agentd, with the work token from the work's secret.
//!
//! Environment: `AGENTD_API_URL`, `AGENTD_ENVIRONMENT_ID`, `AGENTD_WORK_ID`,
//! `AGENTD_WORK_SECRET`; `AGENTD_AGENT_UID` / `AGENTD_AGENT_GID` (65534).
//!
//! `agentd-worker relay` runs the gateway instead (see `relay`).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

mod client;
mod ops;
mod relay;

use std::time::Duration;

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
    let agent = ops::Agent { uid: config.uid, gid: config.gid };
    let client = Client::new(config);
    client.ack_until_done().await;
    let heartbeat = tokio::spawn({
        let client = client.clone();
        async move { client.keep_lease().await }
    });
    let serve = tokio::spawn({
        let client = client.clone();
        async move {
            loop {
                match client.next_op().await {
                    Ok(Some(envelope)) => {
                        let client = client.clone();
                        tokio::spawn(async move {
                            let outcome = ops::run(envelope.op, agent).await;
                            client.complete(&envelope.id, &outcome).await;
                        });
                    }
                    Ok(None) => {}
                    Err(e) => {
                        eprintln!("agentd-worker: polling: {e}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    });
    // The lease ends the worker: stopped work, a lost lease, or agentd gone.
    let reason = heartbeat.await.unwrap_or_else(|e| e.to_string());
    serve.abort();
    eprintln!("agentd-worker: stopping: {reason}");
    client.stop().await;
}
