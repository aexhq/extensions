use std::net::IpAddr;

#[cfg(test)]
pub const SPECIAL_USE_FIXTURES: &[(&str, &str)] = &[
    ("0.0.0.1", "this-network"),
    ("10.0.0.1", "private"),
    ("100.64.0.1", "carrier-grade NAT"),
    ("127.0.0.1", "loopback"),
    ("169.254.169.254", "link-local"),
    ("172.16.0.1", "private"),
    ("192.0.0.170", "IETF protocol assignments"),
    ("192.0.2.1", "documentation"),
    ("192.31.196.1", "AS112"),
    ("192.52.193.1", "AMT relay"),
    ("192.88.99.1", "6to4 relay"),
    ("192.168.0.1", "private"),
    ("192.175.48.1", "AS112"),
    ("198.18.0.1", "benchmarking"),
    ("198.51.100.1", "documentation"),
    ("203.0.113.1", "documentation"),
    ("224.0.0.1", "multicast"),
    ("255.255.255.255", "reserved"),
];

#[cfg(test)]
pub const PUBLIC_UNICAST_FIXTURES: &[&str] = &["1.1.1.1", "8.8.8.8", "93.184.216.34"];

pub fn is_public_unicast(address: &IpAddr) -> bool {
    let IpAddr::V4(address) = address else {
        return false;
    };
    let value = u32::from(*address);
    IPV4_SPECIAL_USE
        .iter()
        .all(|&(network, prefix)| !prefix_match(value, network, prefix))
}

const IPV4_SPECIAL_USE: &[(u32, u8)] = &[
    (0x0000_0000, 8),
    (0x0a00_0000, 8),
    (0x6440_0000, 10),
    (0x7f00_0000, 8),
    (0xa9fe_0000, 16),
    (0xac10_0000, 12),
    (0xc000_0000, 24),
    (0xc000_0200, 24),
    (0xc01f_c400, 24),
    (0xc034_c100, 24),
    (0xc058_6300, 24),
    (0xc0a8_0000, 16),
    (0xc0af_3000, 24),
    (0xc612_0000, 15),
    (0xc633_6400, 24),
    (0xcb00_7100, 24),
    (0xe000_0000, 4),
    (0xf000_0000, 4),
];

fn prefix_match(value: u32, network: u32, prefix: u8) -> bool {
    let mask = u32::MAX.checked_shl(u32::from(32 - prefix)).unwrap_or(0);
    value & mask == network & mask
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn policy_vectors_cover_every_special_range() {
        for &(address, _) in SPECIAL_USE_FIXTURES {
            assert!(!is_public_unicast(&address.parse().unwrap()), "{address}");
        }
        for &address in PUBLIC_UNICAST_FIXTURES {
            assert!(is_public_unicast(&address.parse().unwrap()), "{address}");
        }
        assert!(!is_public_unicast(&IpAddr::V6("::1".parse().unwrap())));
        assert!(is_public_unicast(&IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4))));
    }
}
