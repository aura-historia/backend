//! Shared fail-closed policy for outbound public HTTP destinations.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Returns whether an address is safe for an outbound public HTTP destination.
///
/// This is an SSRF policy, not a claim that every accepted address is globally
/// reachable in every network. IPv6 permits only global-unicast `2000::/3`
/// after excluding known special-purpose ranges.
pub fn is_safe_public_http_destination(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => !IPV4_NON_PUBLIC
            .iter()
            .any(|(network, prefix)| ipv4_in_prefix(address, *network, *prefix)),
        IpAddr::V6(address) => {
            ipv6_in_prefix(address, Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0), 3)
                && !IPV6_NON_PUBLIC
                    .iter()
                    .any(|(network, prefix)| ipv6_in_prefix(address, *network, *prefix))
        }
    }
}

const IPV4_NON_PUBLIC: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(0, 0, 0, 0), 8),
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(127, 0, 0, 0), 8),
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    (Ipv4Addr::new(192, 0, 2, 0), 24),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    (Ipv4Addr::new(198, 51, 100, 0), 24),
    (Ipv4Addr::new(203, 0, 113, 0), 24),
    (Ipv4Addr::new(224, 0, 0, 0), 4),
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

const IPV6_NON_PUBLIC: &[(Ipv6Addr, u8)] = &[
    (Ipv6Addr::UNSPECIFIED, 96),
    (Ipv6Addr::LOCALHOST, 128),
    (Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0, 0), 96),
    (Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0), 96),
    (Ipv6Addr::new(0x64, 0xff9b, 0, 1, 0, 0, 0, 0), 48),
    (Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0), 64),
    (Ipv6Addr::new(0x2001, 0, 0, 1, 0, 0, 0, 0), 64),
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
    (Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
    (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20),
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
    (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
    (Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0), 10),
    (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),
];

fn ipv4_in_prefix(address: Ipv4Addr, network: Ipv4Addr, prefix: u8) -> bool {
    let mask = u32::MAX.checked_shl(u32::from(32 - prefix)).unwrap_or(0);
    (u32::from(address) & mask) == (u32::from(network) & mask)
}

fn ipv6_in_prefix(address: Ipv6Addr, network: Ipv6Addr, prefix: u8) -> bool {
    let mask = u128::MAX.checked_shl(u32::from(128 - prefix)).unwrap_or(0);
    (u128::from(address) & mask) == (u128::from(network) & mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_fail_closed_for_non_global_ipv6_ranges() {
        for address in [
            "4000::1",
            "64:ff9b:1::1",
            "100:0:0:1::1",
            "3fff::1",
            "5f00::1",
        ] {
            assert!(!is_safe_public_http_destination(address.parse().unwrap()));
        }
        assert!(is_safe_public_http_destination(
            "2606:4700:4700::1111".parse().unwrap()
        ));
    }
}
