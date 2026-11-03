//! agentd-worker: the gateway relay. When agentd and the egress service run
//! on the host, agentd starts one container of the sandbox image as
//! `agentd-worker relay`, on the default bridge and on each sandbox's own
//! network, relaying only agentd's API port and the egress proxy's port.
//!
//! Environment: `AGENTD_RELAY` (`port=host:port,...`).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

mod relay;

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() != Some("relay") {
        eprintln!("usage: agentd-worker relay");
        std::process::exit(2);
    }
    match relay::parse(&std::env::var("AGENTD_RELAY").unwrap_or_default()) {
        Ok(pairs) => relay::run(pairs).await,
        Err(e) => eprintln!("agentd-worker: {e}"),
    }
    std::process::exit(1);
}
