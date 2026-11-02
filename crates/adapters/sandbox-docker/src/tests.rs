use std::collections::BTreeMap;

use serde_json::json;
use shared::ports::JobKind;

use super::*;

#[test]
fn limited_networking_gets_no_network() {
    assert_eq!(network_mode(&json!({"networking": {"type": "limited"}})), "none");
    assert_eq!(network_mode(&json!({"networking": {"type": "unrestricted"}})), SANDBOX_NETWORK);
    assert_eq!(network_mode(&serde_json::Value::Null), SANDBOX_NETWORK);
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
    let runtime = DockerRuntime::new(docker, "alpine:3.20", SandboxLimits::default());
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

/// Needs a Docker daemon: `AGENTD_DOCKER_TESTS=1`.
#[tokio::test]
async fn commands_and_files_go_through_the_sandbox_port() {
    use shared::ports::sandbox::{ExecRequest, Owner, Sandbox, SandboxFile};
    if std::env::var("AGENTD_DOCKER_TESTS").is_err() {
        return;
    }
    let docker = Docker::connect_with_local_defaults().unwrap();
    let runtime = DockerRuntime::new(docker, "alpine:3.20", SandboxLimits::default());
    runtime.prepare().await.unwrap();
    let target = format!("test-exec-{}", std::process::id());
    let job = JobSpec {
        kind: JobKind::Session,
        target_id: target.clone(),
        workspace_id: "wrkspc_test".into(),
        environment_id: "env_test".into(),
        environment: json!({"networking": {"type": "limited"}}),
        env: BTreeMap::new(),
    };
    let handle = runtime.launch(job).await.unwrap();
    let secs = std::time::Duration::from_secs(10);

    let out = runtime
        .exec(
            &target,
            ExecRequest::new(["sh", "-c", "cat; id -u; echo err >&2; exit 3"], secs)
                .with_stdin(b"hi\n".to_vec()),
        )
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hi\n65534\n");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "err\n");
    assert_eq!(out.exit_code, 3);

    let upload =
        SandboxFile { path: "/mnt/session/uploads/a/b.json".into(), bytes: b"{}".to_vec(), mode: 0o444 };
    runtime.put_files(&target, vec![upload]).await.unwrap();
    runtime.make_dir(&target, "/mnt/session/outputs", Owner::Agent).await.unwrap();
    let write =
        "cat /mnt/session/uploads/a/b.json > /mnt/session/outputs/r.json && ! touch /mnt/session/uploads/a/x";
    let out = runtime.exec(&target, ExecRequest::new(["sh", "-c", write], secs)).await.unwrap();
    assert_eq!(out.exit_code, 0, "{}", String::from_utf8_lossy(&out.stderr));
    let tree = runtime.read_tree(&target, "/mnt/session/outputs").await.unwrap();
    assert_eq!(tree.get("/mnt/session/outputs/r.json").map(Vec::as_slice), Some(b"{}".as_slice()));
    assert!(runtime.read_tree(&target, "/nope").await.unwrap().is_empty());

    let out = runtime
        .exec(&target, ExecRequest::new(["sleep", "5"], std::time::Duration::from_secs(1)))
        .await
        .unwrap();
    assert!(out.timed_out);
    runtime.stop(&handle).await.unwrap();
}
