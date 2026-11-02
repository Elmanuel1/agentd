//! The sandbox runtime setting and its limits.

use sandbox_docker::{SandboxLimits, SandboxNetwork, GATEWAY};

use super::{number, selector, unknown, ConfigError};

/// The image a Docker sandbox runs when `AGENTD_SANDBOX_IMAGE` is unset:
/// `docker/sandbox` (its first process is `agentd-worker`).
pub const DEFAULT_SANDBOX_IMAGE: &str = "agentd-sandbox";

/// `AGENTD_RUNTIME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeConfig {
    /// `docker`: one container per session from `image`
    /// (`AGENTD_SANDBOX_IMAGE`), on the daemon `DOCKER_HOST` names.
    Docker {
        /// The sandbox image.
        image: String,
        /// `AGENTD_SANDBOX_MEMORY_MIB` (default 2048) and
        /// `AGENTD_SANDBOX_MILLICPUS` (default 2000) per sandbox.
        limits: SandboxLimits,
        /// `AGENTD_SANDBOX_JOIN`: the containers that join the sandboxes'
        /// internal network, agentd's then the egress service's,
        /// comma-separated; unset, a gateway relays agentd's API port and the
        /// egress proxy's port from the host.
        network: SandboxNetwork,
    },
}

/// The port of `addr` (`host:port`), or `default`.
pub(super) fn port(addr: Option<String>, default: u16) -> u16 {
    addr.and_then(|a| a.rsplit(':').next().and_then(|p| p.parse().ok())).unwrap_or(default)
}

/// The containers `AGENTD_SANDBOX_JOIN` names.
fn joined(get: &impl Fn(&str) -> Option<String>) -> Vec<String> {
    get("AGENTD_SANDBOX_JOIN")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Where a sandbox reaches the `index`th joined container (0: agentd, 1: the
/// egress service, which is agentd's when only one is named), or the
/// gateway.
pub(super) fn sandbox_reach(get: &impl Fn(&str) -> Option<String>, index: usize) -> String {
    let joined = joined(get);
    joined.get(index).or(joined.first()).cloned().unwrap_or_else(|| GATEWAY.to_owned())
}

impl RuntimeConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        Ok(match selector(get, "AGENTD_RUNTIME", "docker").as_str() {
            "docker" => Self::Docker {
                image: get("AGENTD_SANDBOX_IMAGE").unwrap_or_else(|| DEFAULT_SANDBOX_IMAGE.into()),
                limits: SandboxLimits {
                    memory_mib: number(get, "AGENTD_SANDBOX_MEMORY_MIB", 2048)?,
                    millicpus: number(get, "AGENTD_SANDBOX_MILLICPUS", 2000)?,
                },
                network: match joined(get) {
                    containers if !containers.is_empty() => SandboxNetwork::Joined { containers },
                    _ => {
                        let (api, egress) =
                            (port(get("AGENTD_ADDR"), 8080), port(get("AGENTD_EGRESS_ADDR"), 8890));
                        let relay = |p: u16| (p, format!("host.docker.internal:{p}"));
                        SandboxNetwork::Gateway { relays: vec![relay(api), relay(egress)] }
                    }
                },
            },
            other => return Err(unknown("AGENTD_RUNTIME", other)),
        })
    }
}
