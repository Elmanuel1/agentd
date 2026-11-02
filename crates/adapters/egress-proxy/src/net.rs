//! Where the proxy may connect: public addresses only, pinned at lookup so
//! a second resolution cannot swap in an internal one.

use std::net::{IpAddr, SocketAddr};

/// `host`'s addresses, refused when any is loopback, private, link-local
/// (the cloud metadata address among them) or otherwise internal.
pub async fn public_addrs(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let addrs: Vec<SocketAddr> =
        tokio::net::lookup_host((host, port)).await.map_err(|e| format!("{host}: {e}"))?.collect();
    if addrs.is_empty() {
        return Err(format!("{host}: no address"));
    }
    if let Some(bad) = addrs.iter().find(|a| internal(a.ip())) {
        return Err(format!("{host} resolves to an internal address ({})", bad.ip()));
    }
    Ok(addrs)
}

/// Whether `ip` is an address no sandbox may reach through the proxy.
pub fn internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64
        }
        IpAddr::V6(ip) => {
            let first = ip.segments().first().copied().unwrap_or_default();
            ip.is_loopback()
                || ip.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || ip.to_ipv4_mapped().is_some_and(|v4| internal(IpAddr::V4(v4)))
        }
    }
}
