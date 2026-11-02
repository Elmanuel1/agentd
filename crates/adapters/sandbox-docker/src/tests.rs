use std::collections::BTreeMap;

use serde_json::json;
use shared::ports::JobKind;

use super::*;

#[test]
fn limited_networking_gets_no_network() {
    assert_eq!(network_mode(&json!({"networking": {"type": "limited"}})), "none");
    assert_eq!(network_mode(&json!({"networking": {"type": "unrestricted"}})), "bridge");
    assert_eq!(network_mode(&serde_json::Value::Null), "bridge");
}

#[test]
fn a_job_runs_in_a_container_named_after_its_target() {
    assert_eq!(DockerRuntime::container_name("sesn_01x"), "agentd-sesn_01x");
}

/// Needs a Docker daemon: `AGENTD_DOCKER_TESTS=1`.
#[tokio::test]
async fn a_container_comes_and_goes() {
    if std::env::var("AGENTD_DOCKER_TESTS").is_err() {
        return;
    }
    let docker = Docker::connect_with_local_defaults().unwrap();
    let runtime = DockerRuntime::new(docker, "alpine:3.20");
    runtime.prepare().await.unwrap();
    let target = format!("test-{}", std::process::id());
    let job = JobSpec {
        kind: JobKind::Session,
        target_id: target.clone(),
        workspace_id: "wrkspc_test".into(),
        environment_id: "env_test".into(),
        environment: json!({"networking": {"type": "limited"}}),
        env: BTreeMap::from([("A".into(), "1".into())]),
    };
    let handle = runtime.launch(job).await.unwrap();
    assert_eq!(Some(&handle), runtime.handle_for(&target).as_ref());
    assert_eq!(runtime.status(&handle).await.unwrap(), TaskState::Running);
    runtime.stop(&handle).await.unwrap();
    assert!(matches!(runtime.status(&handle).await, Err(RuntimeError::Unknown(_))));
    assert!(matches!(runtime.stop(&handle).await, Err(RuntimeError::Unknown(_))));
}
