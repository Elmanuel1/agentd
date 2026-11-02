//! The Docker sandbox: [`DockerRuntime`] runs each job in one container
//! named `agentd-<target id>`, from the configured image, with the network
//! the job's environment asks for.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

use std::collections::HashMap;

use async_trait::async_trait;
use bollard::errors::Error as DockerError;
use bollard::models::{ContainerCreateBody, ContainerStateStatusEnum, HostConfig, NetworkCreateRequest};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, RemoveContainerOptionsBuilder,
};
use bollard::Docker;
use futures::TryStreamExt;
use shared::ports::{JobSpec, Runtime, RuntimeError, RuntimeHandle, TaskState};

mod exec;

/// What one sandbox may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxLimits {
    /// Memory cap in MiB.
    pub memory_mib: i64,
    /// CPU cap, in thousandths of a CPU.
    pub millicpus: i64,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self { memory_mib: 2048, millicpus: 2000 }
    }
}

/// The network sandboxes with outbound access share: a bridge with
/// container-to-container traffic off, so one session cannot reach another.
pub const SANDBOX_NETWORK: &str = "agentd-sandboxes";

/// One container per job, on a Docker daemon.
#[derive(Debug, Clone)]
pub struct DockerRuntime {
    docker: Docker,
    image: String,
    limits: SandboxLimits,
}

impl DockerRuntime {
    /// Run jobs from `image` on `docker`, each within `limits`.
    pub fn new(docker: Docker, image: impl Into<String>, limits: SandboxLimits) -> Self {
        Self { docker, image: image.into(), limits }
    }

    /// The container a job runs in.
    pub fn container_name(target_id: &str) -> String {
        format!("agentd-{target_id}")
    }

    /// Create the sandbox network and pull the image unless the daemon has
    /// them, so a launch never waits on a registry. Run once at boot.
    pub async fn prepare(&self) -> Result<(), RuntimeError> {
        self.ensure_network().await?;
        self.ensure_image().await
    }

    async fn ensure_network(&self) -> Result<(), RuntimeError> {
        match self.docker.inspect_network(SANDBOX_NETWORK, None).await {
            Ok(_) => return Ok(()),
            Err(e) if not_found(&e) => {}
            Err(e) => return Err(RuntimeError::Unavailable(e.to_string())),
        }
        let request = NetworkCreateRequest {
            name: SANDBOX_NETWORK.into(),
            driver: Some("bridge".into()),
            options: Some(HashMap::from([(
                "com.docker.network.bridge.enable_icc".to_owned(),
                "false".to_owned(),
            )])),
            ..Default::default()
        };
        match self.docker.create_network(request).await {
            Ok(_) => Ok(()),
            // Another replica created it first.
            Err(DockerError::DockerResponseServerError { status_code: 409, .. }) => Ok(()),
            Err(e) => Err(RuntimeError::Unavailable(e.to_string())),
        }
    }

    async fn ensure_image(&self) -> Result<(), RuntimeError> {
        match self.docker.inspect_image(&self.image).await {
            Ok(_) => return Ok(()),
            Err(e) if not_found(&e) => {}
            Err(e) => return Err(RuntimeError::Unavailable(e.to_string())),
        }
        let pull = CreateImageOptionsBuilder::default().from_image(&self.image).build();
        self.docker
            .create_image(Some(pull), None, None)
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| RuntimeError::Launch(format!("pull {}: {e}", self.image)))?;
        Ok(())
    }
}

/// The most processes one sandbox may run.
const PIDS_LIMIT: i64 = 1024;

/// Sandboxes run as `nobody`, not root.
const SANDBOX_USER: &str = "65534:65534";

/// `limited` networking gets no network: the allow-list proxy is a later
/// step, and denying all egress is the safe reading of `limited` until then.
/// Everything else goes on the sandbox network, never the default bridge.
fn network_mode(environment: &serde_json::Value) -> &'static str {
    match environment.pointer("/networking/type").and_then(serde_json::Value::as_str) {
        Some("limited") => "none",
        _ => SANDBOX_NETWORK,
    }
}

fn not_found(e: &DockerError) -> bool {
    matches!(e, DockerError::DockerResponseServerError { status_code: 404, .. })
}

#[async_trait]
impl Runtime for DockerRuntime {
    async fn launch(&self, job: JobSpec) -> Result<RuntimeHandle, RuntimeError> {
        let name = Self::container_name(&job.target_id);
        let mut env: Vec<String> = job.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        env.push(format!("AGENTD_TARGET_ID={}", job.target_id));
        let labels = HashMap::from([
            ("agentd.target".to_owned(), job.target_id.clone()),
            ("agentd.workspace".to_owned(), job.workspace_id.clone()),
            ("agentd.environment".to_owned(), job.environment_id.clone()),
        ]);
        let body = ContainerCreateBody {
            image: Some(self.image.clone()),
            cmd: Some(vec!["sleep".into(), "infinity".into()]),
            env: Some(env),
            labels: Some(labels),
            user: Some(SANDBOX_USER.into()),
            host_config: Some(HostConfig {
                network_mode: Some(network_mode(&job.environment).to_owned()),
                init: Some(true),
                cap_drop: Some(vec!["ALL".into()]),
                security_opt: Some(vec!["no-new-privileges".into()]),
                pids_limit: Some(PIDS_LIMIT),
                memory: Some(self.limits.memory_mib * 1024 * 1024),
                nano_cpus: Some(self.limits.millicpus * 1_000_000),
                ..Default::default()
            }),
            ..Default::default()
        };
        let options = CreateContainerOptionsBuilder::default().name(&name).build();
        self.docker
            .create_container(Some(options), body)
            .await
            .map_err(|e| RuntimeError::Launch(e.to_string()))?;
        if let Err(e) = self.docker.start_container(&name, None).await {
            let _ = self.stop(&RuntimeHandle { id: name }).await;
            return Err(RuntimeError::Launch(e.to_string()));
        }
        Ok(RuntimeHandle { id: name })
    }

    async fn status(&self, handle: &RuntimeHandle) -> Result<TaskState, RuntimeError> {
        let info = match self.docker.inspect_container(&handle.id, None).await {
            Ok(info) => info,
            Err(e) if not_found(&e) => return Err(RuntimeError::Unknown(handle.id.clone())),
            Err(e) => return Err(RuntimeError::Unavailable(e.to_string())),
        };
        let state = info.state.unwrap_or_default();
        Ok(match state.status {
            Some(ContainerStateStatusEnum::RUNNING | ContainerStateStatusEnum::PAUSED) => TaskState::Running,
            Some(ContainerStateStatusEnum::CREATED | ContainerStateStatusEnum::RESTARTING) => {
                TaskState::Pending
            }
            Some(ContainerStateStatusEnum::EXITED) => {
                TaskState::Exited(i32::try_from(state.exit_code.unwrap_or_default()).unwrap_or(i32::MAX))
            }
            other => TaskState::Failed(format!("container state {other:?}")),
        })
    }

    fn handle_for(&self, target_id: &str) -> Option<RuntimeHandle> {
        Some(RuntimeHandle { id: Self::container_name(target_id) })
    }

    async fn stop(&self, handle: &RuntimeHandle) -> Result<(), RuntimeError> {
        let options = RemoveContainerOptionsBuilder::default().force(true).build();
        match self.docker.remove_container(&handle.id, Some(options)).await {
            Ok(()) => Ok(()),
            Err(e) if not_found(&e) => Err(RuntimeError::Unknown(handle.id.clone())),
            Err(e) => Err(RuntimeError::Unavailable(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests;
