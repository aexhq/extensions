//! Immutable selection of a plane-provisioned network connector.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::num::NonZeroU16;

use serde::{Deserialize, Serialize};

// The class vocabulary lives in `environment-policy`; this re-export keeps catalog consumers on one
// import path.
pub use environment_policy::connector::ConnectorClass;

/// An opaque platform connector identity. Environments never derives an ARN from tenant input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorRef(String);

impl ConnectorRef {
    pub fn parse(value: impl Into<String>) -> Result<Self, ConnectorError> {
        let value = value.into();
        if value.trim() != value || value.is_empty() || value.chars().any(char::is_whitespace) {
            return Err(ConnectorError::InvalidReference);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The private CONNECT gateway reached by the allowlist connector.
///
/// This is deliberately an authority rather than a URL: credentials and capabilities must be
/// supplied out-of-band and must never become part of process arguments or diagnostic output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayAuthority {
    host: String,
    port: NonZeroU16,
}

impl GatewayAuthority {
    pub fn parse(value: &str) -> Result<Self, ConnectorError> {
        if value.trim() != value
            || value.is_empty()
            || value.chars().any(char::is_whitespace)
            || value.contains("//")
            || value.contains(['@', '/', '?', '#', '[', ']'])
        {
            return Err(ConnectorError::InvalidGatewayAuthority);
        }
        let (host, port) = value
            .rsplit_once(':')
            .ok_or(ConnectorError::InvalidGatewayAuthority)?;
        if host.is_empty() || host.contains(':') || !valid_gateway_host(host) {
            return Err(ConnectorError::InvalidGatewayAuthority);
        }
        let port = port
            .parse::<NonZeroU16>()
            .map_err(|_| ConnectorError::InvalidGatewayAuthority)?;
        Ok(Self {
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub const fn port(&self) -> NonZeroU16 {
        self.port
    }

    #[must_use]
    pub fn as_authority(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn valid_gateway_host(host: &str) -> bool {
    if host.parse::<Ipv4Addr>().is_ok() {
        return true;
    }
    // Reject malformed numeric IPv4-like spellings instead of letting a resolver reinterpret
    // them. DNS authorities are ASCII deployment values; IDNA does not belong on this boundary.
    if host.chars().all(|ch| ch.is_ascii_digit() || ch == '.') {
        return false;
    }
    host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
}

/// The complete connector set for one isolated deployment plane.
#[derive(Debug, Clone)]
pub struct ConnectorCatalog {
    refs: BTreeMap<ConnectorClass, ConnectorRef>,
}

impl ConnectorCatalog {
    pub fn new(none: ConnectorRef, public: ConnectorRef, allowlist: ConnectorRef) -> Self {
        Self {
            refs: BTreeMap::from([
                (ConnectorClass::None, none),
                (ConnectorClass::Public, public),
                (ConnectorClass::Allowlist, allowlist),
            ]),
        }
    }

    /// Resolves only the sealed class. There is deliberately no fallback ordering.
    #[must_use]
    pub fn resolve(&self, class: ConnectorClass) -> &ConnectorRef {
        self.refs
            .get(&class)
            .expect("the constructor installs every connector class")
    }

    pub fn from_lookup(
        mut lookup: impl FnMut(ConnectorClass) -> Option<String>,
    ) -> Result<Self, ConnectorError> {
        let get = |class, lookup: &mut dyn FnMut(ConnectorClass) -> Option<String>| {
            let value = lookup(class).ok_or(ConnectorError::MissingClass(class))?;
            ConnectorRef::parse(value)
        };
        Ok(Self::new(
            get(ConnectorClass::None, &mut lookup)?,
            get(ConnectorClass::Public, &mut lookup)?,
            get(ConnectorClass::Allowlist, &mut lookup)?,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectorError {
    #[error("connector reference must be a non-empty token without whitespace")]
    InvalidReference,
    #[error(
        "egress gateway authority must be an IPv4 address or DNS name followed by a non-zero port; URLs and credentials are forbidden"
    )]
    InvalidGatewayAuthority,
    #[error("deployment plane has no {0:?} connector")]
    MissingClass(ConnectorClass),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector(value: &str) -> ConnectorRef {
        ConnectorRef::parse(value).unwrap()
    }

    #[test]
    fn every_class_resolves_to_exactly_its_configured_connector() {
        let catalog = ConnectorCatalog::new(
            connector("none-arn"),
            connector("public-arn"),
            connector("allowlist-arn"),
        );
        assert_eq!(catalog.resolve(ConnectorClass::None).as_str(), "none-arn");
        assert_eq!(
            catalog.resolve(ConnectorClass::Public).as_str(),
            "public-arn"
        );
        assert_eq!(
            catalog.resolve(ConnectorClass::Allowlist).as_str(),
            "allowlist-arn"
        );
    }

    #[test]
    fn an_incomplete_catalog_fails_instead_of_broadening() {
        let error = ConnectorCatalog::from_lookup(|class| match class {
            ConnectorClass::Public => Some("public-arn".into()),
            _ => None,
        })
        .unwrap_err();
        assert_eq!(error, ConnectorError::MissingClass(ConnectorClass::None));
    }

    #[test]
    fn references_are_closed_tokens() {
        for value in ["", " public", "public ", "public connector", "public\narn"] {
            assert_eq!(
                ConnectorRef::parse(value).unwrap_err(),
                ConnectorError::InvalidReference
            );
        }
    }

    #[test]
    fn gateway_authority_accepts_the_fixed_private_nlb_ipv4() {
        let authority = GatewayAuthority::parse("10.42.0.10:8443").unwrap();
        assert_eq!(authority.host(), "10.42.0.10");
        assert_eq!(authority.port().get(), 8443);
        assert_eq!(authority.as_authority(), "10.42.0.10:8443");
    }

    #[test]
    fn gateway_authority_accepts_dns_but_never_urls_or_credentials() {
        assert_eq!(
            GatewayAuthority::parse("Gateway.Internal:8443")
                .unwrap()
                .as_authority(),
            "gateway.internal:8443"
        );
        for value in [
            "",
            "10.0.0.10",
            "10.0.0.10:0",
            "999.0.0.1:8443",
            "http://10.0.0.10:8443",
            "aex:secret@10.0.0.10:8443",
            "10.0.0.10:8443/path",
            "[::1]:8443",
        ] {
            assert_eq!(
                GatewayAuthority::parse(value).unwrap_err(),
                ConnectorError::InvalidGatewayAuthority
            );
        }
    }
}
