use std::collections::BTreeMap;

use serde_json::json;
use shared::ports::JobKind;

use super::*;

#[test]
fn every_sandbox_goes_on_the_sandbox_network() {
    assert_eq!(network_mode(&json!({"networking": {"type": "limited"}})), SANDBOX_NETWORK);
    assert_eq!(network_mode(&serde_json::Value::Null), SANDBOX_NETWORK);
}

#[test]
fn a_job_runs_in_a_container_named_after_its_target() {
    assert_eq!(DockerRuntime::container_name("sesn_01x"), "agentd-sesn_01x");
}

/// Needs a Docker daemon and the sandbox image (its worker is the
/// container's process): `AGENTD_DOCKER_TESTS=1`, `AGENTD_SANDBOX_IMAGE`.
#[tokio::test]
async fn a_container_comes_and_goes() {
    let (Ok(_), Ok(image)) = (std::env::var("AGENTD_DOCKER_TESTS"), std::env::var("AGENTD_SANDBOX_IMAGE"))
    else {
        return;
    };
    let docker = Docker::connect_with_local_defaults().unwrap();
    let runtime = DockerRuntime::new(docker, image, SandboxLimits::default());
    runtime.prepare().await.unwrap();
    let target = format!("test-{}", std::process::id());
    let job = JobSpec {
        kind: JobKind::Session,
        target_id: target.clone(),
        workspace_id: "wrkspc_test".into(),
        environment_id: "env_test".into(),
        environment: json!({"networking": {"type": "limited"}}),
        // A worker with nowhere to report keeps retrying its ack, so the
        // container stays up.
        env: BTreeMap::from([
            ("AGENTD_API_URL".into(), "http://127.0.0.1:9".into()),
            ("AGENTD_ENVIRONMENT_ID".into(), "env_test".into()),
            ("AGENTD_WORK_ID".into(), "work_test".into()),
            ("AGENTD_WORK_SECRET".into(), "eyJzZXNzaW9uc190b2tlbiI6InQifQ".into()),
        ]),
    };
    let handle = runtime.launch(job).await.unwrap();
    assert_eq!(Some(&handle), runtime.handle_for(&target).as_ref());
    assert_eq!(runtime.status(&handle).await.unwrap(), TaskState::Running);
    runtime.stop(&handle).await.unwrap();
    assert!(matches!(runtime.status(&handle).await, Err(RuntimeError::Unknown(_))));
    assert!(matches!(runtime.stop(&handle).await, Err(RuntimeError::Unknown(_))));
}
