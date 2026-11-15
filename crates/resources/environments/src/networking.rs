//! Which hosts an environment allows: the one reading of its networking
//! policy, for the agent's web tools, its sandbox and its MCP servers.

use shared::net::host_matches;

use crate::types::{EnvironmentConfig, Networking};

/// What `allow_package_managers` opens under `limited` networking.
pub const PACKAGE_HOSTS: [&str; 9] = [
    "pypi.org",
    "files.pythonhosted.org",
    "registry.npmjs.org",
    "deb.debian.org",
    "security.debian.org",
    "dl-cdn.alpinelinux.org",
    "crates.io",
    "index.crates.io",
    "static.crates.io",
];

impl Networking {
    /// The hosts a `limited` policy allows, with the package registries when
    /// it allows those; `None` when unrestricted.
    pub fn allowed_hosts(&self) -> Option<Vec<String>> {
        match self {
            Self::Unrestricted => None,
            Self::Limited { allowed_hosts, allow_package_managers, .. } => {
                let mut hosts = allowed_hosts.clone();
                if *allow_package_managers {
                    hosts.extend(PACKAGE_HOSTS.iter().map(|h| (*h).to_owned()));
                }
                Some(hosts)
            }
        }
    }
}

impl EnvironmentConfig {
    /// Whether the agent's MCP server at `host` may be reached: always under
    /// `unrestricted` networking or a `self_hosted` environment; under
    /// `limited`, when `allow_mcp_servers` is set or `allowed_hosts` lists it.
    pub fn mcp_allowed(&self, host: &str) -> bool {
        match self {
            Self::Cloud {
                networking: Networking::Limited { allowed_hosts, allow_mcp_servers, .. }, ..
            } => *allow_mcp_servers || allowed_hosts.iter().any(|rule| host_matches(rule, host)),
            Self::Cloud { networking: Networking::Unrestricted, .. } | Self::SelfHosted {} => true,
        }
    }
}

/// [`Networking::allowed_hosts`] of an environment `config` as JSON. A
/// config with no policy is unrestricted, as its default is; one whose policy
/// does not read allows no host (fail closed).
pub fn allowed_hosts(config: &serde_json::Value) -> Option<Vec<String>> {
    match config.get("networking") {
        None | Some(serde_json::Value::Null) => None,
        Some(policy) => match serde_json::from_value::<Networking>(policy.clone()) {
            Ok(networking) => networking.allowed_hosts(),
            Err(_) => Some(Vec::new()),
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_limited_policy_lists_its_hosts_and_the_registries_it_allows() {
        assert_eq!(allowed_hosts(&json!({"networking": {"type": "unrestricted"}})), None);
        assert_eq!(allowed_hosts(&json!({})), None);
        let only = json!({"networking": {"type": "limited", "allowed_hosts": ["api.example.com"]}});
        assert_eq!(allowed_hosts(&only), Some(vec!["api.example.com".to_owned()]));
        let registries = json!({"networking": {"type": "limited", "allow_package_managers": true}});
        assert!(allowed_hosts(&registries).unwrap().contains(&"pypi.org".to_owned()));
        let unreadable = json!({"networking": {"type": "limited", "allowed_hosts": "everything"}});
        assert_eq!(allowed_hosts(&unreadable), Some(vec![]));
    }

    #[test]
    fn mcp_hosts_pass_under_unrestricted_or_when_allowed_or_listed() {
        let config = |networking: serde_json::Value| -> EnvironmentConfig {
            serde_json::from_value(json!({"type": "cloud", "networking": networking})).unwrap()
        };
        assert!(config(json!({"type": "unrestricted"})).mcp_allowed("mcp.example.com"));
        let closed = config(json!({"type": "limited", "allowed_hosts": ["*.example.com"]}));
        assert!(closed.mcp_allowed("mcp.example.com"));
        assert!(!closed.mcp_allowed("mcp.other.com"));
        assert!(config(json!({"type": "limited", "allow_mcp_servers": true})).mcp_allowed("mcp.other.com"));
        assert!(EnvironmentConfig::SelfHosted {}.mcp_allowed("mcp.other.com"));
    }
}
