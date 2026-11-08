use std::collections::BTreeMap;

use serde_json::json;
use shared::ports::JobKind;

use super::*;

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
    let relays = vec![(18080, "host.docker.internal:18080".to_owned())];
    let runtime =
        DockerRuntime::new(docker, image, SandboxLimits::default(), SandboxNetwork::Gateway { relays });
    runtime.prepare().await.unwrap();
    let target = format!("test-{}", std::process::id());
    let job = JobSpec {
        kind: JobKind::Session,
        target_id: target.clone(),
        workspace_id: "wrkspc_test".into(),
        environment_id: "env_test".into(),
        environment: json!({"networking": {"type": "limited"}}),
        // A worker with nowhere to report keeps retrying the session, so the
        // container stays up.
        env: BTreeMap::from([
            ("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:9".into()),
            ("ANTHROPIC_ENVIRONMENT_ID".into(), "env_test".into()),
            ("ANTHROPIC_SESSION_ID".into(), "sesn_test".into()),
            ("ANTHROPIC_WORK_ID".into(), "work_test".into()),
            ("ANTHROPIC_WORK_SECRET".into(), "eyJzZXNzaW9uc190b2tlbiI6InQifQ".into()),
        ]),
    };
    let handle = runtime.launch(job).await.unwrap();
    assert_eq!(Some(&handle), runtime.handle_for(&target).as_ref());
    assert_eq!(runtime.status(&handle).await.unwrap(), TaskState::Running);
    // The sandbox's own internal network, shared with the gateway only.
    let net = network_name(&target);
    let network = runtime.docker.inspect_network(&net, None).await.unwrap();
    assert_eq!(network.internal, Some(true));
    let members: Vec<String> =
        network.containers.unwrap_or_default().into_values().filter_map(|c| c.name).collect();
    assert_eq!(members.len(), 2, "{members:?}");
    assert!(members.contains(&GATEWAY.to_owned()));
    let container =
        runtime.docker.inspect_container(&DockerRuntime::container_name(&target), None).await.unwrap();
    let mode = container.host_config.and_then(|h| h.network_mode);
    assert_eq!(mode.as_deref(), Some(net.as_str()));
    runtime.stop(&handle).await.unwrap();
    assert!(matches!(runtime.status(&handle).await, Err(RuntimeError::Unknown(_))));
    assert!(matches!(runtime.stop(&handle).await, Err(RuntimeError::Unknown(_))));
    assert!(runtime.docker.inspect_network(&network_name(&target), None).await.is_err(), "the network stays");
}
