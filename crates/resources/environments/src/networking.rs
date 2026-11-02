//! Which hosts an environment's sandbox may reach: the one reading of its
//! networking policy, for the agent's web tools and the egress proxy alike.

use crate::types::Networking;

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

/// [`Networking::allowed_hosts`] of an environment `config` as JSON; a
/// config without a readable policy is unrestricted, as its default is.
pub fn allowed_hosts(config: &serde_json::Value) -> Option<Vec<String>> {
    let policy = config.get("networking").cloned().unwrap_or_default();
    serde_json::from_value::<Networking>(policy).unwrap_or_default().allowed_hosts()
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
    }
}
