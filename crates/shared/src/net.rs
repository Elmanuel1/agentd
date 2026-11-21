//! Where agentd itself may connect on a tenant's behalf (an MCP server, an
//! OAuth token endpoint): public addresses only, pinned at lookup so a second
//! resolution cannot swap in an internal one, with redirects never followed.
//! A client is reused for its host for 5 minutes; then the host is resolved
//! and checked again.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use crate::ttl::TtlMap;

/// Why a guarded connection was refused before any byte was sent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    /// The URL does not parse or names no host.
    #[error("invalid URL")]
    Url,
    /// The host has no address, or its lookup failed.
    #[error("the host could not be resolved")]
    Unresolved,
    /// The host resolves to an internal address.
    #[error("{0} resolves to an internal address")]
    Internal(String),
    /// The HTTP client could not be built.
    #[error("http client: {0}")]
    Client(String),
}

/// The `user-agent` live's MCP client sends.
const USER_AGENT: &str = "Claude-User";
/// How long a client is reused before its host is resolved and checked again.
const CLIENT_TTL: Duration = Duration::from_secs(300);
/// Clients kept per guard.
const CLIENTS: usize = 1_024;

/// A cached client's host, port, timeout and user agent.
type ClientKey = (String, u16, Duration, &'static str);

/// Builds HTTP clients pinned to a host's public addresses. `allowed` names
/// internal addresses that may still be reached: none in production, the
/// test servers' own sockets in tests. Clones share the client cache.
#[derive(Debug, Clone)]
pub struct Guard {
    allowed: Vec<SocketAddr>,
    user_agent: &'static str,
    clients: Arc<TtlMap<ClientKey, reqwest::Client>>,
}

impl Default for Guard {
    fn default() -> Self {
        Self::allowing(Vec::new())
    }
}

impl Guard {
    /// A guard that also lets `allowed` through.
    pub fn allowing(allowed: Vec<SocketAddr>) -> Self {
        let clients = Arc::new(TtlMap::new(CLIENT_TTL, CLIENTS));
        Self { allowed, user_agent: USER_AGENT, clients }
    }

    /// The same guard sending `user_agent`, with a cache of its own.
    pub fn with_user_agent(&self, user_agent: &'static str) -> Self {
        Self { user_agent, ..Self::allowing(self.allowed.clone()) }
    }

    /// `url`'s host and port, resolved and checked.
    pub async fn resolve(&self, url: &reqwest::Url) -> Result<(String, Vec<SocketAddr>), NetError> {
        let (host, port) = host_port(url)?;
        let addrs: Vec<SocketAddr> =
            tokio::net::lookup_host((host.as_str(), port)).await.map_err(|_| NetError::Unresolved)?.collect();
        if addrs.is_empty() {
            return Err(NetError::Unresolved);
        }
        if let Some(bad) = addrs.iter().find(|a| internal(a.ip()) && !self.allowed.contains(a)) {
            return Err(NetError::Internal(bad.ip().to_string()));
        }
        Ok((host, addrs))
    }

    /// A client for `url`'s host: pinned to the addresses checked when it
    /// was built, no redirects, `timeout` per request, sending the guard's
    /// `user-agent` (live's MCP client: `Claude-User`).
    pub async fn client(&self, url: &reqwest::Url, timeout: Duration) -> Result<reqwest::Client, NetError> {
        let (host, port) = host_port(url)?;
        let key = (host, port, timeout, self.user_agent);
        if let Some(client) = self.clients.get(&key) {
            return Ok(client);
        }
        let (host, addrs) = self.resolve(url).await?;
        let client = reqwest::Client::builder()
            .user_agent(self.user_agent)
            .resolve_to_addrs(&host, &addrs)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .map_err(|e| NetError::Client(e.to_string()))?;
        self.clients.insert(key, client.clone());
        Ok(client)
    }
}

fn host_port(url: &reqwest::Url) -> Result<(String, u16), NetError> {
    let host = url.host_str().ok_or(NetError::Url)?.trim_matches(['[', ']']).to_owned();
    Ok((host, url.port_or_known_default().ok_or(NetError::Url)?))
}

/// The host `url` names, when it parses.
pub fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url).ok()?.host_str().map(|h| h.trim_matches(['[', ']']).to_owned())
}

/// Whether `host` matches `rule`: a bare host matches only itself;
/// `*.example.com` matches every subdomain of `example.com`, not the apex.
pub fn host_matches(rule: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let rule = rule.trim_end_matches('.').to_ascii_lowercase();
    match rule.strip_prefix("*.") {
        Some(apex) => host.len() > apex.len() + 1 && host.ends_with(&format!(".{apex}")),
        None => rule == host,
    }
}

/// Whether `ip` is an address agentd never reaches for a tenant: private,
/// loopback, link-local (the cloud metadata address among them), shared,
/// reserved, benchmarking, multicast or unspecified, and any IPv6 address
/// that embeds one (mapped, compatible, 6to4, NAT64).
pub fn internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_multicast()
                || a == 0
                || (a == 100 && (b & 0xc0) == 64)
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (b & 0xfe) == 18)
                || a >= 240
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            let first = s.first().copied().unwrap_or_default();
            let embedded = |hi: u16, lo: u16| {
                IpAddr::V4(std::net::Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo)))
            };
            let [_, _, _, _, _, _, g, h] = s;
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || ip.to_ipv4_mapped().is_some_and(|v4| internal(IpAddr::V4(v4)))
                // IPv4-compatible ::a.b.c.d
                || (s.get(..6) == Some(&[0; 6]) && internal(embedded(g, h)))
                // NAT64 64:ff9b::/96 and 64:ff9b:1::/48
                || (first == 0x64 && s.get(1) == Some(&0xff9b))
                // 6to4 2002::/16 embeds an IPv4 address in its next 32 bits
                || (first == 0x2002 && internal(embedded(s.get(1).copied().unwrap_or(0), s.get(2).copied().unwrap_or(0))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_internal_range_is_refused_and_public_ones_pass() {
        for bad in [
            "10.0.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.1.2.3",
            "192.0.0.8",
            "198.18.0.1",
            "240.0.0.1",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:169.254.169.254",
            "::169.254.169.254",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::1",
            "2002:a9fe:a9fe::1",
            "ff02::1",
        ] {
            assert!(internal(bad.parse().unwrap()), "{bad}");
        }
        for good in ["8.8.8.8", "140.82.112.3", "2606:4700::1111", "2002:808:808::1"] {
            assert!(!internal(good.parse().unwrap()), "{good}");
        }
    }

    #[test]
    fn a_wildcard_rule_matches_subdomains_only() {
        assert!(host_matches("api.example.com", "API.example.com."));
        assert!(host_matches("*.example.com", "a.example.com"));
        assert!(!host_matches("*.example.com", "example.com"));
        assert!(!host_matches("example.com", "a.example.com"));
    }

    #[tokio::test]
    async fn a_loopback_url_is_refused_unless_its_socket_is_allowed() {
        let url: reqwest::Url = "http://127.0.0.1:9/mcp".parse().unwrap();
        assert_eq!(Guard::default().resolve(&url).await, Err(NetError::Internal("127.0.0.1".into())));
        let allowed = Guard::allowing(vec!["127.0.0.1:9".parse().unwrap()]);
        assert!(allowed.resolve(&url).await.is_ok());
        let other: reqwest::Url = "http://127.0.0.1:10/mcp".parse().unwrap();
        assert!(allowed.resolve(&other).await.is_err());
        let metadata: reqwest::Url = "http://169.254.169.254/latest".parse().unwrap();
        assert!(matches!(allowed.resolve(&metadata).await, Err(NetError::Internal(_))));
    }

    /// The guard's client for `url`, and whether it came from the cache.
    async fn cached(guard: &Guard, url: &reqwest::Url) -> bool {
        let key = (url.host_str().unwrap().to_owned(), url.port().unwrap(), TIMEOUT, guard.user_agent);
        let hit = guard.clients.get(&key).is_some();
        guard.client(url, TIMEOUT).await.unwrap();
        hit
    }

    const TIMEOUT: Duration = Duration::from_secs(10);

    #[tokio::test(start_paused = true)]
    async fn two_calls_to_one_host_reuse_a_client_and_after_5_min_the_address_is_checked_again() {
        let url: reqwest::Url = "http://127.0.0.1:9/mcp".parse().unwrap();
        let guard = Guard::allowing(vec!["127.0.0.1:9".parse().unwrap()]);
        assert!(!cached(&guard, &url).await);
        let other: reqwest::Url = "http://127.0.0.1:9/other".parse().unwrap();
        assert!(cached(&guard.clone(), &other).await, "a clone shares the cache");
        tokio::time::advance(CLIENT_TTL).await;
        assert!(!cached(&guard, &url).await, "resolved and checked again");
        let platform = guard.with_user_agent("Claude-Platform");
        assert!(!cached(&platform, &url).await, "another user agent builds its own");
    }
}
