use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub const MAX_PORTS_PER_DESTINATION: usize = 32;

use brain_protocol::network::is_public_unicast;
use hickory_resolver::TokioResolver;
use hickory_resolver::proto::rr::{RData, Record};
use ipnet::Ipv4Net;

use crate::capability::{CapabilityDestination, DestinationProtocol, VerifiedCapability};

#[derive(Debug, Clone)]
pub struct DenyPolicy {
    hosts: Vec<String>,
    cidrs: Vec<Ipv4Net>,
}

impl DenyPolicy {
    #[must_use]
    pub fn new(hosts: Vec<String>, cidrs: Vec<Ipv4Net>) -> Self {
        Self { hosts, cidrs }
    }

    fn denies_host(&self, host: &str) -> bool {
        self.hosts.iter().any(|pattern| host_matches(pattern, host))
    }

    fn denies_ip(&self, address: Ipv4Addr) -> bool {
        self.cidrs.iter().any(|cidr| cidr.contains(&address))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedPolicy {
    destinations: Vec<ValidatedDestination>,
}

#[derive(Debug, Clone)]
enum ValidatedDestination {
    Host { pattern: String, port: u16 },
    Cidr { cidr: Ipv4Net, port: u16 },
}

impl ValidatedPolicy {
    pub(crate) fn new(destinations: &[CapabilityDestination]) -> Result<Self, PolicyError> {
        let mut validated = Vec::new();
        for destination in destinations {
            if destination.ports.is_empty() || destination.ports.len() > MAX_PORTS_PER_DESTINATION {
                return Err(PolicyError::MalformedPolicy);
            }
            let mut ports = destination.ports.clone();
            ports.sort_unstable();
            ports.dedup();
            if ports.len() != destination.ports.len() || ports.contains(&0) {
                return Err(PolicyError::MalformedPolicy);
            }
            match (&destination.host, &destination.cidr, destination.protocol) {
                (Some(host), None, DestinationProtocol::Tls) if ports == [443] => {
                    let pattern = normalize_host_pattern(host)?;
                    validated.push(ValidatedDestination::Host { pattern, port: 443 });
                }
                (None, Some(cidr), DestinationProtocol::Tcp) => {
                    for port in ports {
                        validated.push(ValidatedDestination::Cidr { cidr: *cidr, port });
                    }
                }
                _ => return Err(PolicyError::MalformedPolicy),
            }
        }
        Ok(Self {
            destinations: validated,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AuthorizedTarget {
    pub address: SocketAddr,
    pub expected_sni: Option<String>,
}

pub async fn authorize(
    capability: &VerifiedCapability,
    deny: &DenyPolicy,
    resolver: &TokioResolver,
    authority: &str,
    port: u16,
) -> Result<AuthorizedTarget, PolicyError> {
    if port == 0 {
        return Err(PolicyError::MalformedTarget);
    }
    if let Ok(address) = authority.parse::<IpAddr>() {
        let IpAddr::V4(address) = address else {
            return Err(PolicyError::Ipv6Unsupported);
        };
        check_ip(deny, address)?;
        let allowed = capability.policy.destinations.iter().any(|destination| {
            matches!(destination, ValidatedDestination::Cidr { cidr, port: allowed_port }
                if *allowed_port == port && cidr.contains(&address))
        });
        if !allowed {
            return Err(PolicyError::Denied);
        }
        return Ok(AuthorizedTarget {
            address: SocketAddr::new(address.into(), port),
            expected_sni: None,
        });
    }

    let host = normalize_host(authority)?;
    if deny.denies_host(&host) {
        return Err(PolicyError::PermanentDeny);
    }
    let host_rule = capability.policy.destinations.iter().any(|destination| {
        matches!(destination, ValidatedDestination::Host { pattern, port: allowed_port }
            if *allowed_port == port && host_matches(pattern, &host))
    });
    let response = resolver
        // A trailing dot prevents a deployment search suffix from changing the signed host.
        .lookup_ip(format!("{host}."))
        .await
        .map_err(|_| PolicyError::ResolutionFailed)?;
    let mut addresses = checked_ipv4_answers(deny, response.as_lookup().answers())?;
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        return Err(PolicyError::ResolutionFailed);
    }
    let cidr_rule = addresses.iter().all(|address| {
        capability.policy.destinations.iter().any(|destination| {
            matches!(destination, ValidatedDestination::Cidr { cidr, port: allowed_port }
                if *allowed_port == port && cidr.contains(address))
        })
    });
    if !host_rule && !cidr_rule {
        return Err(PolicyError::Denied);
    }
    Ok(AuthorizedTarget {
        address: SocketAddr::new(addresses[0].into(), port),
        expected_sni: host_rule.then_some(host),
    })
}

fn checked_ipv4_answers(
    deny: &DenyPolicy,
    records: &[Record],
) -> Result<Vec<Ipv4Addr>, PolicyError> {
    let mut addresses = Vec::new();
    for record in records {
        match &record.data {
            RData::CNAME(cname) => {
                let alias = normalize_dns_name(&cname.0.to_ascii())?;
                if deny.denies_host(&alias) {
                    return Err(PolicyError::PermanentDeny);
                }
            }
            RData::A(address) => {
                let address = address.0;
                check_ip(deny, address)?;
                addresses.push(address);
            }
            RData::AAAA(_) => return Err(PolicyError::Ipv6Unsupported),
            _ => {}
        }
    }
    Ok(addresses)
}

fn normalize_dns_name(value: &str) -> Result<String, PolicyError> {
    normalize_host(value.trim_end_matches('.'))
}

fn check_ip(deny: &DenyPolicy, address: Ipv4Addr) -> Result<(), PolicyError> {
    if !is_public_unicast(&IpAddr::V4(address)) || deny.denies_ip(address) {
        Err(PolicyError::PermanentDeny)
    } else {
        Ok(())
    }
}

pub(crate) fn normalize_host_pattern(value: &str) -> Result<String, PolicyError> {
    if let Some(suffix) = value.strip_prefix("*.") {
        let suffix = normalize_host(suffix)?;
        if !suffix.contains('.') {
            return Err(PolicyError::MalformedPolicy);
        }
        Ok(format!("*.{suffix}"))
    } else {
        normalize_host(value)
    }
}

pub(crate) fn normalize_host(value: &str) -> Result<String, PolicyError> {
    if value.is_empty()
        || value.len() > 253
        || value.starts_with('.')
        || value.ends_with('.')
        || value.contains('*')
        || value.chars().any(char::is_whitespace)
    {
        return Err(PolicyError::MalformedTarget);
    }
    let ascii = idna::domain_to_ascii_strict(value).map_err(|_| PolicyError::MalformedTarget)?;
    let ascii = ascii.to_ascii_lowercase();
    if ascii.is_empty()
        || ascii
            .split('.')
            .any(|label| label.is_empty() || label.len() > 63)
    {
        return Err(PolicyError::MalformedTarget);
    }
    Ok(ascii)
}

fn host_matches(pattern: &str, host: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("*.") {
        host.len() > suffix.len() + 1
            && host.ends_with(suffix)
            && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
    } else {
        pattern == host
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("destination policy is malformed")]
    MalformedPolicy,
    #[error("destination is malformed")]
    MalformedTarget,
    #[error("IPv6 is not supported")]
    Ipv6Unsupported,
    #[error("destination is permanently denied")]
    PermanentDeny,
    #[error("destination is outside the capability")]
    Denied,
    #[error("destination resolution failed")]
    ResolutionFailed,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use hickory_resolver::proto::rr::rdata::{A, CNAME};
    use hickory_resolver::proto::rr::{Name, Record};

    use super::*;

    #[test]
    fn host_patterns_are_idna_normalized_and_boundary_safe() {
        assert_eq!(normalize_host("EXAMPLE.com").unwrap(), "example.com");
        assert!(host_matches("*.example.com", "api.example.com"));
        assert!(host_matches("*.example.com", "deep.api.example.com"));
        assert!(!host_matches("*.example.com", "example.com"));
        assert!(!host_matches("*.example.com", "notexample.com"));
        assert!(normalize_host_pattern("*.com").is_err());
    }

    #[test]
    fn policy_rejects_host_tcp_cidr_tls_duplicates_and_zero_ports() {
        for destination in [
            CapabilityDestination {
                host: Some("example.com".into()),
                cidr: None,
                ports: vec![80],
                protocol: DestinationProtocol::Tcp,
            },
            CapabilityDestination {
                host: None,
                cidr: Some("8.8.8.0/24".parse().unwrap()),
                ports: vec![443],
                protocol: DestinationProtocol::Tls,
            },
            CapabilityDestination {
                host: Some("example.com".into()),
                cidr: None,
                ports: vec![443, 443],
                protocol: DestinationProtocol::Tls,
            },
            CapabilityDestination {
                host: None,
                cidr: Some("8.8.8.0/24".parse().unwrap()),
                ports: vec![0],
                protocol: DestinationProtocol::Tcp,
            },
        ] {
            assert_eq!(
                ValidatedPolicy::new(&[destination]).unwrap_err(),
                PolicyError::MalformedPolicy
            );
        }
    }

    #[test]
    fn every_cname_and_final_address_is_checked_against_permanent_denies() {
        let deny = DenyPolicy::new(
            vec!["metadata.aex.dev".into()],
            vec!["10.0.0.0/8".parse().unwrap()],
        );
        let denied_alias = vec![
            Record::from_rdata(
                Name::from_str("allowed.example.").unwrap(),
                60,
                RData::CNAME(CNAME(Name::from_str("metadata.aex.dev.").unwrap())),
            ),
            Record::from_rdata(
                Name::from_str("metadata.aex.dev.").unwrap(),
                60,
                RData::A(A::new(8, 8, 8, 8)),
            ),
        ];
        assert_eq!(
            checked_ipv4_answers(&deny, &denied_alias),
            Err(PolicyError::PermanentDeny)
        );

        let denied_address = vec![Record::from_rdata(
            Name::from_str("allowed.example.").unwrap(),
            60,
            RData::A(A::new(10, 0, 0, 1)),
        )];
        assert_eq!(
            checked_ipv4_answers(&deny, &denied_address),
            Err(PolicyError::PermanentDeny)
        );
    }

    #[test]
    fn special_use_classifier_matches_the_cross_boundary_golden_vectors() {
        for &(address, _) in brain_protocol::network::SPECIAL_USE_FIXTURES {
            let address: IpAddr = address.parse().unwrap();
            if address.is_ipv4() {
                assert!(!is_public_unicast(&address), "{address} must be denied");
            }
        }

        for &address in brain_protocol::network::PUBLIC_UNICAST_FIXTURES {
            let address: IpAddr = address.parse().unwrap();
            if address.is_ipv4() {
                assert!(is_public_unicast(&address), "{address} must remain public");
            }
        }
    }
}
