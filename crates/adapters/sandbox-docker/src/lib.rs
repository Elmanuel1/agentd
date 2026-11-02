//! The Docker sandbox: [`DockerRuntime`] runs each job in one container
//! named `agentd-<target id>`, from the configured image, with the network
//! the job's environment asks for.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

use std::collections::HashMap;

mod network;

pub use network::{network_name, SandboxNetwork, GATEWAY};

use async_trait::async_trait;
use bollard::errors::Error as DockerError;
use bollard::models::{ContainerCreateBody, ContainerStateStatusEnum, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, RemoveContainerOptionsBuilder,
};
use bollard::Docker;
use futures::TryStreamExt;
use shared::ports::{JobSpec, Runtime, RuntimeError, RuntimeHandle, TaskState};

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

/// One container per job, on a Docker daemon.
#[derive(Debug, Clone)]
pub struct DockerRuntime {
    docker: Docker,
    image: String,
    limits: SandboxLimits,
    /// How agentd and the egress service are on each sandbox's network.
    network: SandboxNetwork,
}

impl DockerRuntime {
    /// Run jobs from `image` on `docker`, each within `limits`, on a network
    /// of its own that agentd and the egress service join as `network` says.
    pub fn new(
        docker: Docker,
        image: impl Into<String>,
        limits: SandboxLimits,
        network: SandboxNetwork,
    ) -> Self {
        Self { docker, image: image.into(), limits, network }
    }

    /// The container a job runs in.
    pub fn container_name(target_id: &str) -> String {
        format!("agentd-{target_id}")
    }

    /// Pull the image unless the daemon has it, so a launch never waits on a
    /// registry, and start the gateway when there is one. Run once at boot.
    pub async fn prepare(&self) -> Result<(), RuntimeError> {
        self.ensure_image().await?;
        self.members().await.map(|_| ())
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

/// The sandbox's first process is its worker, as root so it can write the
/// root-owned uploads and skills; it runs every agent command as `nobody`.
const WORKER_USER: &str = "0:0";

/// What the worker needs beyond an unprivileged process: to run commands as
/// the agent's user and to own files for it.
const WORKER_CAPS: [&str; 5] = ["SETUID", "SETGID", "CHOWN", "DAC_OVERRIDE", "FOWNER"];

fn not_found(e: &DockerError) -> bool {
    matches!(e, DockerError::DockerResponseServerError { status_code: 404, .. })
}

#[async_trait]
impl Runtime for DockerRuntime {
    async fn launch(&self, job: JobSpec) -> Result<RuntimeHandle, RuntimeError> {
        let name = Self::container_name(&job.target_id);
        let network = self.open_network(&job.target_id).await?;
        let mut env: Vec<String> = job.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        env.push(format!("AGENTD_TARGET_ID={}", job.target_id));
        let labels = HashMap::from([
            ("agentd.target".to_owned(), job.target_id.clone()),
            ("agentd.workspace".to_owned(), job.workspace_id.clone()),
            ("agentd.environment".to_owned(), job.environment_id.clone()),
        ]);
        let body = ContainerCreateBody {
            image: Some(self.image.clone()),
            cmd: Some(vec!["agentd-worker".into()]),
            env: Some(env),
            labels: Some(labels),
            user: Some(WORKER_USER.into()),
            host_config: Some(HostConfig {
                network_mode: Some(network),
                init: Some(true),
                cap_drop: Some(vec!["ALL".into()]),
                cap_add: Some(WORKER_CAPS.iter().map(|c| (*c).to_owned()).collect()),
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
        let removed = match self.docker.remove_container(&handle.id, Some(options)).await {
            Ok(()) => Ok(()),
            Err(e) if not_found(&e) => Err(RuntimeError::Unknown(handle.id.clone())),
            Err(e) => return Err(RuntimeError::Unavailable(e.to_string())),
        };
        if let Some(target) = handle.id.strip_prefix("agentd-") {
            self.close_network(target).await;
        }
        removed
    }
}

#[cfg(test)]
mod tests;
