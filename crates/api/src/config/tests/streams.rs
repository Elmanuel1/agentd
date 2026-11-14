//! Streams, retention, the read replica, the role and the loop's settings.

use super::*;

#[test]
fn valkey_is_required_and_events_are_kept_unless_configured() {
    let c = config(&[]).unwrap();
    let base = StreamsConfig {
        valkey_url: "redis://valkey:6379".into(),
        valkey_cluster: false,
        changes_prefix: "agentd".into(),
        retention: None,
    };
    assert_eq!(c.streams, base);
    assert_eq!(config(&[("AGENTD_VALKEY_URL", "")]).unwrap_err().var, "AGENTD_VALKEY_URL");
    let StoreConfig::Postgres { read_url, .. } = &c.store;
    assert_eq!(read_url, &None);
}

#[test]
fn valkey_a_read_replica_and_retention_come_from_config() {
    let c = config(&[
        ("AGENTD_VALKEY_URL", "rediss://:pw@valkey:6379"),
        ("AGENTD_VALKEY_CLUSTER", "true"),
        ("AGENTD_PG_READ_URL", "postgres://reader:r3ad-pass@replica:5432/agentd"),
        ("AGENTD_EVENT_RETENTION_DAYS", "30"),
    ])
    .unwrap();
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    assert_eq!(
        c.streams,
        StreamsConfig {
            valkey_url: "rediss://:pw@valkey:6379".into(),
            valkey_cluster: true,
            changes_prefix: "agentd".into(),
            retention: Some(day * 30)
        }
    );
    let StoreConfig::Postgres { read_url, .. } = &c.store;
    assert_eq!(read_url.as_deref(), Some("postgres://reader:r3ad-pass@replica:5432/agentd"));
    let shown = format!("{c:?}");
    for secret in [":pw@", "r3ad-pass", "s3cr3t-pass"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    assert!(shown.contains("***@replica:5432/agentd"), "{shown}");
}

#[test]
fn a_retention_period_is_a_positive_number_of_days() {
    for bad in ["0", "-1", "week"] {
        let err = config(&[("AGENTD_EVENT_RETENTION_DAYS", bad)]).unwrap_err();
        assert_eq!(err.var, "AGENTD_EVENT_RETENTION_DAYS");
    }
}

#[test]
fn the_role_and_the_loop_s_settings_come_from_config() {
    let c = config(&[]).unwrap();
    assert_eq!(c.agent_loop.role, Role::All);
    assert!(c.agent_loop.role.runs_loop() && c.agent_loop.role.serves_api());
    assert_eq!((c.agent_loop.per_workspace, c.agent_loop.attempts, c.agent_loop.steps), (32, 5, 256));
    let api = config(&[("AGENTD_ROLE", "api")]).unwrap();
    assert!(!api.agent_loop.role.runs_loop(), "an api replica claims no step");
    let worker = config(&[("AGENTD_ROLE", "loop")]).unwrap();
    assert!(!worker.agent_loop.role.serves_api());
    assert_eq!(worker.agent_loop.model_base_url, "https://openrouter.ai/api/v1");
    assert_eq!(config(&[("AGENTD_ROLE", "worker")]).unwrap_err().var, "AGENTD_ROLE");
}
