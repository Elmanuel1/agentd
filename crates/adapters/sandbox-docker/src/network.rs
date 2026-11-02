//! The network sandboxes run on. Their worker must reach agentd; nothing
//! else should be reachable except through agentd's egress proxy.
//!
//! Sandboxes always sit on an `internal` network with container-to-container
//! traffic off: no route out, so every request goes through the egress
//! proxy, and the host and the cloud metadata address are out of reach.
//! agentd is on that network in one of two ways ([`SandboxNetwork`]):
//!
//! - `Joined`: agentd runs in a container, which joins the network.
//! - `Gateway`: agentd runs on the host (development, CI). A gateway
//!   container, on the network and on the default bridge, relays only
//!   agentd's API and proxy ports to the host.

use std::collections::HashMap;

use bollard::errors::Error as DockerError;
use bollard::models::{
    ContainerCreateBody, HostConfig, NetworkConnectRequest, NetworkCreateRequest, RestartPolicy,
    RestartPolicyNameEnum,
};
use bollard::query_parameters::CreateContainerOptionsBuilder;
use shared::ports::RuntimeError;

use super::{not_found, DockerRuntime, SANDBOX_NETWORK};

/// The gateway container's name; sandboxes reach agentd through it.
pub const GATEWAY: &str = "agentd-gateway";

/// How agentd is on the sandbox network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxNetwork {
    /// agentd's own container joins it.
    Joined {
        /// agentd's container.
        container: String,
    },
    /// A gateway relays these `(port, host:port)` pairs to agentd.
    Gateway {
        /// The ports relayed.
        relays: Vec<(u16, String)>,
    },
}

impl DockerRuntime {
    /// Create the sandbox network unless it exists, and attach agentd to it
    /// when isolating.
    pub(super) async fn ensure_network(&self) -> Result<(), RuntimeError> {
        match self.docker.inspect_network(SANDBOX_NETWORK, None).await {
            Ok(_) => {}
            Err(e) if not_found(&e) => self.create_network().await?,
            Err(e) => return Err(RuntimeError::Unavailable(e.to_string())),
        }
        let member = match &self.network {
            SandboxNetwork::Joined { container } => container.clone(),
            SandboxNetwork::Gateway { relays } => self.ensure_gateway(relays).await?,
        };
        let connect = NetworkConnectRequest { container: member.clone(), ..Default::default() };
        match self.docker.connect_network(SANDBOX_NETWORK, connect).await {
            Ok(()) => Ok(()),
            // Already a member (a restart, or another replica's container).
            Err(DockerError::DockerResponseServerError { status_code: 403 | 409, .. }) => Ok(()),
            Err(e) => Err(RuntimeError::Unavailable(format!("attach {member} to {SANDBOX_NETWORK}: {e}"))),
        }
    }

    /// Start the gateway unless it runs; its name.
    async fn ensure_gateway(&self, relays: &[(u16, String)]) -> Result<String, RuntimeError> {
        let spec: Vec<String> = relays.iter().map(|(p, t)| format!("{p}={t}")).collect();
        let body = ContainerCreateBody {
            image: Some(self.image.clone()),
            cmd: Some(vec!["agentd-worker".into(), "relay".into()]),
            env: Some(vec![format!("AGENTD_RELAY={}", spec.join(","))]),
            user: Some("65534:65534".into()),
            host_config: Some(HostConfig {
                network_mode: Some("bridge".into()),
                extra_hosts: Some(vec!["host.docker.internal:host-gateway".into()]),
                cap_drop: Some(vec!["ALL".into()]),
                security_opt: Some(vec!["no-new-privileges".into()]),
                restart_policy: Some(RestartPolicy {
                    name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let options = CreateContainerOptionsBuilder::default().name(GATEWAY).build();
        match self.docker.create_container(Some(options), body).await {
            Ok(_) | Err(DockerError::DockerResponseServerError { status_code: 409, .. }) => {}
            Err(e) => return Err(RuntimeError::Unavailable(format!("gateway: {e}"))),
        }
        match self.docker.start_container(GATEWAY, None).await {
            Ok(()) | Err(DockerError::DockerResponseServerError { status_code: 304, .. }) => {
                Ok(GATEWAY.to_owned())
            }
            Err(e) => Err(RuntimeError::Unavailable(format!("gateway: {e}"))),
        }
    }

    async fn create_network(&self) -> Result<(), RuntimeError> {
        let request = NetworkCreateRequest {
            name: SANDBOX_NETWORK.into(),
            driver: Some("bridge".into()),
            internal: Some(true),
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
}
