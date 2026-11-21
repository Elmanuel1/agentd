use super::*;

#[test]
fn the_scheduler_is_postgres_unless_temporal_is_chosen() {
    assert_eq!(config(&[]).unwrap().scheduler, SchedulerConfig::Postgres);
    let defaults = config(&[("AGENTD_SCHEDULER", "temporal")]).unwrap().scheduler;
    let want = scheduler_temporal::TemporalConfig {
        address: "http://localhost:7233".into(),
        namespace: "default".into(),
        task_queue: "agentd".into(),
        api_key: None,
        server_ca: None,
    };
    assert_eq!(defaults, SchedulerConfig::Temporal(want));
    let set = config(&[
        ("AGENTD_SCHEDULER", "temporal"),
        ("AGENTD_TEMPORAL_ADDRESS", "http://temporal:7233"),
        ("AGENTD_TEMPORAL_NAMESPACE", "agents"),
        ("AGENTD_TEMPORAL_TASK_QUEUE", "fires"),
    ])
    .unwrap()
    .scheduler;
    let want = scheduler_temporal::TemporalConfig {
        address: "http://temporal:7233".into(),
        namespace: "agents".into(),
        task_queue: "fires".into(),
        api_key: None,
        server_ca: None,
    };
    assert_eq!(set, SchedulerConfig::Temporal(want));
    assert_eq!(config(&[("AGENTD_SCHEDULER", "eventbridge")]).unwrap_err().var, "AGENTD_SCHEDULER");
}

#[test]
fn temporal_over_tls_takes_an_api_key_and_a_ca() {
    let set = config(&[
        ("AGENTD_SCHEDULER", "temporal"),
        ("AGENTD_TEMPORAL_ADDRESS", "https://agents.acct.tmprl.cloud:7233"),
        ("AGENTD_TEMPORAL_API_KEY", "tmprl-key"),
        ("AGENTD_TEMPORAL_TLS_SERVER_CA_CERT_PATH", "/etc/temporal/ca.pem"),
    ])
    .unwrap()
    .scheduler;
    let SchedulerConfig::Temporal(temporal) = set else { panic!("temporal") };
    assert_eq!(temporal.api_key.as_deref(), Some("tmprl-key"));
    assert_eq!(temporal.server_ca.as_deref(), Some("/etc/temporal/ca.pem"));
    for (var, value) in
        [("AGENTD_TEMPORAL_API_KEY", "k"), ("AGENTD_TEMPORAL_TLS_SERVER_CA_CERT_PATH", "/ca.pem")]
    {
        let err = config(&[("AGENTD_SCHEDULER", "temporal"), (var, value)]).unwrap_err();
        assert_eq!((err.var, err.reason.as_str()), (var, "needs an https:// AGENTD_TEMPORAL_ADDRESS"));
    }
}
