//! The sandbox runtime setting and its limits.

use sandbox_docker::SandboxLimits;

use super::{number, selector, unknown, ConfigError};

/// The image a Docker sandbox runs when `AGENTD_SANDBOX_IMAGE` is unset.
pub const DEFAULT_SANDBOX_IMAGE: &str = "alpine:3.20";

/// `AGENTD_RUNTIME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeConfig {
    /// `none`: no sandbox; sessions run without one.
    None,
    /// `docker`: one container per session from `image`
    /// (`AGENTD_SANDBOX_IMAGE`), on the daemon `DOCKER_HOST` names.
    Docker {
        /// The sandbox image.
        image: String,
        /// `AGENTD_SANDBOX_MEMORY_MIB` (default 2048) and
        /// `AGENTD_SANDBOX_MILLICPUS` (default 2000) per sandbox.
        limits: SandboxLimits,
    },
}

impl RuntimeConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        Ok(match selector(get, "AGENTD_RUNTIME", "none").as_str() {
            "none" => Self::None,
            "docker" => Self::Docker {
                image: get("AGENTD_SANDBOX_IMAGE").unwrap_or_else(|| DEFAULT_SANDBOX_IMAGE.into()),
                limits: SandboxLimits {
                    memory_mib: number(get, "AGENTD_SANDBOX_MEMORY_MIB", 2048)?,
                    millicpus: number(get, "AGENTD_SANDBOX_MILLICPUS", 2000)?,
                },
            },
            other => return Err(unknown("AGENTD_RUNTIME", other)),
        })
    }
}
