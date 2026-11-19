use super::*;

#[test]
fn the_scheduler_is_postgres_unless_temporal_is_chosen() {
    assert_eq!(config(&[]).unwrap().scheduler, SchedulerConfig::Postgres);
    let defaults = config(&[("AGENTD_SCHEDULER", "temporal")]).unwrap().scheduler;
    let want = scheduler_temporal::TemporalConfig {
        address: "http://localhost:7233".into(),
        namespace: "default".into(),
        task_queue: "agentd".into(),
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
    };
    assert_eq!(set, SchedulerConfig::Temporal(want));
    assert_eq!(config(&[("AGENTD_SCHEDULER", "eventbridge")]).unwrap_err().var, "AGENTD_SCHEDULER");
}
