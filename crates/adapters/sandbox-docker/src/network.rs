//! The network each sandbox runs on. Its worker must reach agentd; nothing
//! else should be reachable except through the egress proxy.
//!
//! Each sandbox has an `internal` network of its own: no route out, so every
//! request goes through the egress proxy, and the host and the cloud metadata
//! address are out of reach. Only agentd and the egress service share it, so
//! no other session's sandbox can be reached. They are on it in one of two
//! ways ([`SandboxNetwork`]):
//!
//! - `Joined`: agentd and the egress service run in containers, which join
//!   each sandbox's network.
//! - `Gateway`: they run on the host (development, CI). A gateway
//!   container, on the default bridge and on each sandbox's network, relays
//!   only agentd's API port and the egress proxy's port to the host.

use std::collections::HashMap;

use bollard::errors::Error as DockerError;
use bollard::models::{
    ContainerCreateBody, HostConfig, Ipam, IpamConfig, NetworkConnectRequest, NetworkCreateRequest,
    NetworkDisconnectRequest, RestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::CreateContainerOptionsBuilder;
use shared::ports::RuntimeError;

use super::{not_found, DockerRuntime};

/// The gateway container's name; sandboxes reach agentd through it.
pub const GATEWAY: &str = "agentd-gateway";

/// How many subnets a new network tries before giving up: each is a random
/// /28 of 10.213.0.0/16, and one in use is skipped.
const SUBNET_TRIES: usize = 16;

/// How agentd and the egress service are on each sandbox's network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxNetwork {
    /// agentd's container and the egress service's join it.
    Joined {
        /// The containers.
        containers: Vec<String>,
    },
    /// A gateway relays these `(port, host:port)` pairs to agentd.
    Gateway {
        /// The ports relayed.
        relays: Vec<(u16, String)>,
    },
}

/// The network of the sandbox `target_id` runs in.
pub fn network_name(target_id: &str) -> String {
    format!("agentd-net-{target_id}")
}

impl DockerRuntime {
    /// The containers that join every sandbox's network, the gateway started
    /// when it is one of them.
    pub(super) async fn members(&self) -> Result<Vec<String>, RuntimeError> {
        match &self.network {
            SandboxNetwork::Joined { containers } => Ok(containers.clone()),
            SandboxNetwork::Gateway { relays } => Ok(vec![self.ensure_gateway(relays).await?]),
        }
    }

    /// Create `target_id`'s network unless it exists, with agentd and the
    /// egress service on it; its name.
    pub(super) async fn open_network(&self, target_id: &str) -> Result<String, RuntimeError> {
        let name = network_name(target_id);
        match self.docker.inspect_network(&name, None).await {
            Ok(_) => {}
            Err(e) if not_found(&e) => self.create_network(&name).await?,
            Err(e) => return Err(RuntimeError::Unavailable(e.to_string())),
        }
        for member in self.members().await? {
            let connect = NetworkConnectRequest { container: member.clone(), ..Default::default() };
            match self.docker.connect_network(&name, connect).await {
                // Already a member: a relaunch.
                Ok(()) | Err(DockerError::DockerResponseServerError { status_code: 403 | 409, .. }) => {}
                Err(e) => return Err(RuntimeError::Launch(format!("attach {member} to {name}: {e}"))),
            }
        }
        Ok(name)
    }

    /// Take everyone off `target_id`'s network and remove it. Best effort: a
    /// network left behind holds no container and is reused by a relaunch.
    pub(super) async fn close_network(&self, target_id: &str) {
        let name = network_name(target_id);
        if let Ok(members) = self.members().await {
            for container in members {
                let leave = NetworkDisconnectRequest { container, force: Some(true) };
                let _ = self.docker.disconnect_network(&name, leave).await;
            }
        }
        let _ = self.docker.remove_network(&name).await;
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

    /// An internal network on a free /28. Container-to-container traffic is
    /// on: the sandbox must reach agentd and the egress service, and it is the
    /// only sandbox there.
    async fn create_network(&self, name: &str) -> Result<(), RuntimeError> {
        let mut last = String::new();
        for _ in 0..SUBNET_TRIES {
            let (third, fourth) = (rand::random::<u8>(), rand::random::<u8>() & 0xf0);
            let subnet = format!("10.213.{third}.{fourth}/28");
            let request = NetworkCreateRequest {
                name: name.to_owned(),
                driver: Some("bridge".into()),
                internal: Some(true),
                ipam: Some(Ipam {
                    config: Some(vec![IpamConfig { subnet: Some(subnet), ..Default::default() }]),
                    ..Default::default()
                }),
                labels: Some(HashMap::from([("agentd.sandbox-network".to_owned(), "true".to_owned())])),
                ..Default::default()
            };
            match self.docker.create_network(request).await {
                // Created, or made by an earlier attempt at this launch.
                Ok(_) | Err(DockerError::DockerResponseServerError { status_code: 409, .. }) => return Ok(()),
                // The subnet overlaps one in use: try another.
                Err(DockerError::DockerResponseServerError { status_code: 400 | 403, message }) => {
                    last = message
                }
                Err(e) => return Err(RuntimeError::Launch(e.to_string())),
            }
        }
        Err(RuntimeError::Launch(format!("no free subnet for {name}: {last}")))
    }
}
