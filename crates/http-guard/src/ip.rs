//! Classifies IP addresses as public or not (ported from linkgo
//! `net/destination.rs`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// True for any IPv4 address that is not a globally routable unicast address:
/// loopback, private, link-local (incl. cloud metadata), CGNAT, unspecified,
/// broadcast, multicast, documentation, benchmarking and reserved ranges.
#[must_use]
pub fn is_non_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || (a == 100 && (64..=127).contains(&b)) // CGNAT 100.64.0.0/10
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 198 && (b == 18 || b == 19)) // benchmarking 198.18.0.0/15
        || a >= 240 // reserved 240.0.0.0/4
}

/// True for any IPv6 address that is not globally routable, including
/// IPv6 forms that embed a non-public IPv4 address (mapped, NAT64, SIIT,
/// 6to4) and tunnel prefixes that can reach arbitrary IPv4 hosts.
#[must_use]
pub fn is_non_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_non_public_ipv4(v4);
    }
    let segments = ip.segments();
    let embedded_low = |high: u16, low: u16| {
        let [a, b] = high.to_be_bytes();
        let [c, d] = low.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    // NAT64 well-known prefix 64:ff9b::/96 embeds an IPv4 address.
    if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_non_public_ipv4(embedded_low(segments[6], segments[7]));
    }
    // IPv4-translated ::ffff:0:a.b.c.d (SIIT) embeds an IPv4 address.
    if segments[..6] == [0, 0, 0, 0, 0xffff, 0] {
        return is_non_public_ipv4(embedded_low(segments[6], segments[7]));
    }
    // 6to4 2002:AABB:CCDD::/48 embeds an IPv4 address.
    if segments[0] == 0x2002 {
        return is_non_public_ipv4(embedded_low(segments[1], segments[2]));
    }
    ip.is_loopback()
        || (segments[0] == 0x64 && segments[1] == 0xff9b && segments[2] == 1) // local-use NAT64 64:ff9b:1::/48
        || (segments[0] == 0x2001 && segments[1] == 0) // Teredo 2001::/32 (tunnels to arbitrary IPv4)
        || ip.is_unspecified()
        || ip.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
        || (segments[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        || (segments[0] & 0xffc0) == 0xfec0 // deprecated site-local fec0::/10
        || (segments[0] == 0x2001 && segments[1] == 0x0db8) // documentation
        || segments[..6] == [0, 0, 0, 0, 0, 0] // IPv4-compatible (deprecated)
}

/// True for any address the public-only resolver must drop.
#[must_use]
pub fn is_non_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_non_public_ipv4(v4),
        IpAddr::V6(v6) => is_non_public_ipv6(v6),
    }
}

#[cfg(test)]
mod tests {
    use super::is_non_public_ip;
    use std::net::IpAddr;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn non_public_addresses_are_detected() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "100.64.0.1",      // CGNAT
            "169.254.169.254", // cloud metadata
            "0.0.0.0",
            "255.255.255.255",
            "198.18.0.1",
            "240.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.1.1", // IPv4-mapped
            "64:ff9b::a00:1",     // NAT64 embedding 10.0.0.1
            "64:ff9b:1::1",       // local-use NAT64
            "2002:a00:1::",       // 6to4 embedding 10.0.0.1
            "2001::1",            // Teredo
            "2001:db8::1",        // documentation
        ] {
            assert!(
                is_non_public_ip(ip(address)),
                "{address} must be non-public"
            );
        }
    }

    #[test]
    fn public_addresses_are_allowed() {
        for address in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "64:ff9b::808:808", // NAT64 embedding 8.8.8.8
            "::ffff:8.8.8.8",   // IPv4-mapped public
            "2002:808:808::",   // 6to4 embedding 8.8.8.8
        ] {
            assert!(!is_non_public_ip(ip(address)), "{address} must be public");
        }
    }
}
