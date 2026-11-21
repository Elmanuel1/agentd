//! The Temporal sink over TLS: a [`TlsFront`] serving the test certificate
//! (`docker/tls/certs.sh`, its CA at `AGENTD_TEMPORAL_TLS_CA`) before the dev
//! server at `AGENTD_TEMPORAL_ADDRESS`; skipped without them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::ToSocketAddrs;
use std::path::Path;

use chrono::Utc;
use scheduler_temporal::{TemporalConfig, TemporalScheduler};
use shared::ports::{FireTarget, ScheduleSpec, SchedulerPort, TenantRef};
use test_support::TlsFront;

/// The front, and the config that reaches it, trusting `ca` when given.
async fn over_tls(ca: Option<String>) -> Option<(TlsFront, TemporalConfig)> {
    let (plain, test_ca) =
        (std::env::var("AGENTD_TEMPORAL_ADDRESS").ok()?, std::env::var("AGENTD_TEMPORAL_TLS_CA").ok()?);
    let upstream =
        plain.trim_start_matches("http://").to_socket_addrs().unwrap().find(std::net::SocketAddr::is_ipv4);
    let front = TlsFront::start(Path::new(&test_ca), upstream).await.unwrap();
    let config = TemporalConfig {
        address: format!("https://localhost:{}", front.addr.port()),
        namespace: "default".into(),
        task_queue: format!("agentd-tls-test-{}", Utc::now().timestamp_micros()),
        api_key: Some("test-api-key".into()),
        server_ca: ca.map(|_| test_ca),
    };
    Some((front, config))
}

#[tokio::test]
async fn https_with_the_ca_stores_a_schedule() {
    let Some((front, config)) = over_tls(Some(String::new())).await else { return };
    let sink = TemporalScheduler::connect(config).await.unwrap();
    let target = FireTarget::new("temporaltlstest", format!("t{}", Utc::now().timestamp_micros()));
    let tenant =
        TenantRef { org_id: "org_t".into(), workspace_id: "wrkspc_t".into(), api_key_id: "apikey_t".into() };
    let spec =
        ScheduleSpec { cron: "0 3 * * *".into(), timezone: "UTC".into(), target: target.clone(), tenant };
    sink.upsert(spec).await.unwrap();
    assert!(sink.describe(&target).await.is_ok());
    sink.delete(&target).await.unwrap();
    assert!(front.handshakes() >= 1);
}

#[tokio::test]
async fn https_without_the_ca_refuses_the_test_certificate() {
    let Some((front, config)) = over_tls(None).await else { return };
    let err = TemporalScheduler::connect(config).await.unwrap_err();
    assert!(err.to_string().contains("UnknownIssuer") || err.to_string().contains("certificate"), "{err}");
    assert_eq!(front.handshakes(), 0);
}
