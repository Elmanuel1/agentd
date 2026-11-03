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

/// Whether `ip` is an address no sandbox may reach through the proxy:
/// private, loopback, link-local (the cloud metadata address among them),
/// shared, reserved, benchmarking, multicast or unspecified, and any IPv6
/// address that embeds one (mapped, compatible, 6to4, NAT64).
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
}
