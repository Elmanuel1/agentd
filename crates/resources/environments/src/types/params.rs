//! Environment configuration as a request sends it. Every field may be left
//! out or `null`; on update, a field left out keeps its stored value and
//! `null` takes its default.

use std::collections::BTreeMap;

use serde::Deserialize;
use shared::{ApiError, Patch};

use super::{EnvironmentConfig, Networking, Packages, PackagesTag};

/// A request's `config`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigParams {
    /// Anthropic cloud.
    Cloud {
        /// Network policy.
        #[serde(default)]
        networking: Patch<NetworkingParams>,
        /// Packages (boxed: the one large field).
        #[serde(default)]
        packages: Box<Patch<PackagesParams>>,
        /// Init script.
        #[serde(default)]
        init_script: Patch<String>,
        /// Container environment variables.
        #[serde(default)]
        environment: Patch<BTreeMap<String, String>>,
    },
    /// Self-hosted.
    SelfHosted {},
}

/// A request's network policy.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetworkingParams {
    /// Full outbound access.
    Unrestricted {},
    /// Limited access; each field `false` or `[]` by default.
    Limited {
        /// Hosts the container can reach.
        #[serde(default)]
        allowed_hosts: Patch<Vec<String>>,
        /// Package registries reachable.
        #[serde(default)]
        allow_package_managers: Patch<bool>,
        /// MCP servers reachable.
        #[serde(default)]
        allow_mcp_servers: Patch<bool>,
    },
}

/// A request's packages; each list `[]` by default.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackagesParams {
    /// Always `packages` when sent.
    #[serde(rename = "type", default)]
    pub tag: Option<PackagesTag>,
    /// Ubuntu/Debian packages.
    #[serde(default)]
    pub apt: Patch<Vec<String>>,
    /// Rust packages.
    #[serde(default)]
    pub cargo: Patch<Vec<String>>,
    /// Ruby packages.
    #[serde(default)]
    pub gem: Patch<Vec<String>>,
    /// Go packages.
    #[serde(default)]
    pub go: Patch<Vec<String>>,
    /// Node.js packages.
    #[serde(default)]
    pub npm: Patch<Vec<String>>,
    /// Python packages.
    #[serde(default)]
    pub pip: Patch<Vec<String>>,
}

impl ConfigParams {
    /// The configuration after this request, over `stored` (`None` on create).
    /// A config of another type than the stored one replaces it.
    pub fn apply(self, stored: Option<&EnvironmentConfig>) -> Result<EnvironmentConfig, ApiError> {
        let config = match self {
            Self::SelfHosted {} => EnvironmentConfig::SelfHosted {},
            Self::Cloud { networking, packages, init_script, environment } => {
                let base = match stored {
                    Some(EnvironmentConfig::Cloud { packages, networking, init_script, environment }) => {
                        (packages.clone(), networking.clone(), init_script.clone(), environment.clone())
                    }
                    _ => Default::default(),
                };
                let (s_packages, s_networking, s_init, s_env) = base;
                EnvironmentConfig::Cloud {
                    packages: match *packages {
                        Patch::Keep => s_packages,
                        Patch::Clear => Packages::default(),
                        Patch::Set(p) => p.apply(s_packages),
                    },
                    networking: match networking {
                        Patch::Keep => s_networking,
                        Patch::Clear => Networking::default(),
                        Patch::Set(n) => n.apply(s_networking),
                    },
                    init_script: init_script.apply(s_init, String::new),
                    environment: environment.apply(s_env, BTreeMap::new),
                }
            }
        };
        validate(&config)?;
        Ok(config)
    }
}

impl NetworkingParams {
    fn apply(self, stored: Networking) -> Networking {
        match self {
            Self::Unrestricted {} => Networking::Unrestricted,
            Self::Limited { allowed_hosts, allow_package_managers, allow_mcp_servers } => {
                let (hosts, managers, mcp) = match stored {
                    Networking::Limited { allowed_hosts, allow_package_managers, allow_mcp_servers } => {
                        (allowed_hosts, allow_package_managers, allow_mcp_servers)
                    }
                    Networking::Unrestricted => (Vec::new(), false, false),
                };
                Networking::Limited {
                    allowed_hosts: allowed_hosts.apply(hosts, Vec::new),
                    allow_package_managers: allow_package_managers.apply(managers, || false),
                    allow_mcp_servers: allow_mcp_servers.apply(mcp, || false),
                }
            }
        }
    }
}

impl PackagesParams {
    fn apply(self, stored: Packages) -> Packages {
        Packages {
            tag: PackagesTag::Packages,
            apt: self.apt.apply(stored.apt, Vec::new),
            cargo: self.cargo.apply(stored.cargo, Vec::new),
            gem: self.gem.apply(stored.gem, Vec::new),
            go: self.go.apply(stored.go, Vec::new),
            npm: self.npm.apply(stored.npm, Vec::new),
            pip: self.pip.apply(stored.pip, Vec::new),
        }
    }
}

/// Under `limited` networking, packages need `allow_package_managers`.
fn validate(config: &EnvironmentConfig) -> Result<(), ApiError> {
    if let EnvironmentConfig::Cloud {
        packages,
        networking: Networking::Limited { allow_package_managers: false, .. },
        ..
    } = config
    {
        let p = packages;
        if [&p.apt, &p.cargo, &p.gem, &p.go, &p.npm, &p.pip].iter().any(|l| !l.is_empty()) {
            return Err(ApiError::invalid_request(
                "config: packages require networking.allow_package_managers to be true under limited networking",
            ));
        }
    }
    Ok(())
}
