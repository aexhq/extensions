use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hickory_resolver::TokioResolver;
use hickory_resolver::config::LookupIpStrategy;
use p256::ecdsa::VerifyingKey;
use p256::pkcs8::DecodePublicKey as _;

use crate::policy::{DenyPolicy, normalize_host_pattern};

const ENV_LISTEN: &str = "AEX_GATEWAY_LISTEN";
const ENV_HEALTH_LISTEN: &str = "AEX_GATEWAY_HEALTH_LISTEN";
const ENV_PUBLIC_KEY_FILE: &str = "AEX_GATEWAY_PUBLIC_KEY_FILE";
const ENV_PUBLIC_KEY_PEM: &str = "AEX_GATEWAY_PUBLIC_KEY_PEM";
const ENV_PUBLIC_KEY_DER_BASE64: &str = "AEX_GATEWAY_PUBLIC_KEY_DER_BASE64";
const ENV_DENY_HOSTS: &str = "AEX_GATEWAY_DENY_HOSTS";
const ENV_DENY_CIDRS: &str = "AEX_GATEWAY_DENY_CIDRS";
const ENV_MAX_CONNECTIONS: &str = "AEX_GATEWAY_MAX_CONNECTIONS";
const ENV_MAX_CONNECTIONS_PER_ROOT: &str = "AEX_GATEWAY_MAX_CONNECTIONS_PER_ROOT";
const ENV_MAX_PENDING_SETUPS: &str = "AEX_GATEWAY_MAX_PENDING_SETUPS";
const ENV_MAX_RELAY_BYTES: &str = "AEX_GATEWAY_MAX_RELAY_BYTES";
const ENV_SETUP_TIMEOUT_MS: &str = "AEX_GATEWAY_SETUP_TIMEOUT_MS";
const ENV_IDLE_TIMEOUT_MS: &str = "AEX_GATEWAY_IDLE_TIMEOUT_MS";

#[derive(Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub health_listen: SocketAddr,
    pub public_key: VerifyingKey,
    pub resolver: TokioResolver,
    pub deny: DenyPolicy,
    pub max_connections: usize,
    pub max_connections_per_root: usize,
    pub max_pending_setups: usize,
    pub max_relay_bytes: u64,
    pub setup_timeout: Duration,
    pub idle_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let listen = std::env::var(ENV_LISTEN)
            .unwrap_or_else(|_| "0.0.0.0:8080".into())
            .parse()
            .map_err(|_| ConfigError::Invalid(ENV_LISTEN))?;
        let health_listen = std::env::var(ENV_HEALTH_LISTEN)
            .unwrap_or_else(|_| "0.0.0.0:8081".into())
            .parse()
            .map_err(|_| ConfigError::Invalid(ENV_HEALTH_LISTEN))?;
        if listen == health_listen {
            return Err(ConfigError::Invalid(ENV_HEALTH_LISTEN));
        }
        let public_key = public_key_from_env()?;
        let mut resolver = TokioResolver::builder_tokio()
            .map_err(|error| ConfigError::Resolver(error.to_string()))?;
        resolver.options_mut().ip_strategy = LookupIpStrategy::Ipv4Only;
        // The policy engine checks every alias as well as every final address. Discarding CNAME
        // intermediates would turn a denied internal hostname behind an allowed public alias into
        // a policy bypass.
        resolver.options_mut().preserve_intermediates = true;
        let resolver = resolver
            .build()
            .map_err(|error| ConfigError::Resolver(error.to_string()))?;
        let deny_hosts =
            std::env::var(ENV_DENY_HOSTS).map_err(|_| ConfigError::Missing(ENV_DENY_HOSTS))?;
        let configured_deny_hosts = split_values(&deny_hosts)
            .map(normalize_host_pattern)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ConfigError::Invalid(ENV_DENY_HOSTS))?;
        if configured_deny_hosts.is_empty() {
            return Err(ConfigError::Invalid(ENV_DENY_HOSTS));
        }
        // These are product invariants, not deployment conventions. A malformed or incomplete
        // environment must never make Aex's own public control plane reachable from a sandbox.
        let mut deny_hosts = builtin_deny_hosts();
        deny_hosts.extend(configured_deny_hosts);
        let mut deny_cidrs = Vec::new();
        if let Some(value) = optional_env(ENV_DENY_CIDRS)? {
            for cidr in split_values(&value) {
                deny_cidrs.push(
                    cidr.parse()
                        .map_err(|_| ConfigError::Invalid(ENV_DENY_CIDRS))?,
                );
            }
        }
        Ok(Self {
            listen,
            health_listen,
            public_key,
            resolver,
            deny: DenyPolicy::new(deny_hosts, deny_cidrs),
            max_connections: usize_env(ENV_MAX_CONNECTIONS, 1024, 1, 100_000)?,
            max_connections_per_root: usize_env(ENV_MAX_CONNECTIONS_PER_ROOT, 16, 1, 1_024)?,
            max_pending_setups: usize_env(ENV_MAX_PENDING_SETUPS, 256, 1, 4_096)?,
            max_relay_bytes: u64_env(
                ENV_MAX_RELAY_BYTES,
                2 * 1024 * 1024 * 1024,
                1024 * 1024,
                16 * 1024 * 1024 * 1024,
            )?,
            setup_timeout: Duration::from_millis(u64_env(
                ENV_SETUP_TIMEOUT_MS,
                10_000,
                100,
                60_000,
            )?),
            idle_timeout: Duration::from_millis(u64_env(
                ENV_IDLE_TIMEOUT_MS,
                300_000,
                1_000,
                3_600_000,
            )?),
        })
    }
}

fn builtin_deny_hosts() -> Vec<String> {
    ["aex.dev", "*.aex.dev"]
        .into_iter()
        .map(|host| normalize_host_pattern(host).expect("valid built-in host deny"))
        .collect()
}

fn public_key_from_env() -> Result<VerifyingKey, ConfigError> {
    let file = std::env::var(ENV_PUBLIC_KEY_FILE).ok();
    let pem = std::env::var(ENV_PUBLIC_KEY_PEM).ok();
    let der = std::env::var(ENV_PUBLIC_KEY_DER_BASE64).ok();
    if usize::from(file.is_some()) + usize::from(pem.is_some()) + usize::from(der.is_some()) != 1 {
        return Err(ConfigError::PublicKeySource);
    }
    if let Some(file) = file {
        return read_public_key(&PathBuf::from(file));
    }
    if let Some(pem) = pem {
        return VerifyingKey::from_public_key_pem(&pem)
            .map_err(|error| ConfigError::PublicKey(error.to_string()));
    }
    let der = STANDARD
        .decode(der.expect("exactly one source is present"))
        .map_err(|error| ConfigError::PublicKey(error.to_string()))?;
    VerifyingKey::from_public_key_der(&der)
        .map_err(|error| ConfigError::PublicKey(error.to_string()))
}

fn read_public_key(path: &Path) -> Result<VerifyingKey, ConfigError> {
    let bytes = std::fs::read(path).map_err(|error| ConfigError::PublicKey(error.to_string()))?;
    if bytes.starts_with(b"-----BEGIN") {
        let pem = std::str::from_utf8(&bytes)
            .map_err(|error| ConfigError::PublicKey(error.to_string()))?;
        VerifyingKey::from_public_key_pem(pem)
            .map_err(|error| ConfigError::PublicKey(error.to_string()))
    } else {
        VerifyingKey::from_public_key_der(&bytes)
            .map_err(|error| ConfigError::PublicKey(error.to_string()))
    }
}

fn split_values(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Reads an optional variable, failing closed when a value is present but not UTF-8 instead of
/// silently treating it as unset.
fn optional_env(name: &'static str) -> Result<Option<String>, ConfigError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid(name)),
    }
}

fn usize_env(
    name: &'static str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, ConfigError> {
    let value = match optional_env(name)? {
        Some(value) => usize::from_str(&value).map_err(|_| ConfigError::Invalid(name))?,
        None => default,
    };
    if !(min..=max).contains(&value) {
        return Err(ConfigError::Invalid(name));
    }
    Ok(value)
}

fn u64_env(name: &'static str, default: u64, min: u64, max: u64) -> Result<u64, ConfigError> {
    let value = match optional_env(name)? {
        Some(value) => u64::from_str(&value).map_err(|_| ConfigError::Invalid(name))?,
        None => default,
    };
    if !(min..=max).contains(&value) {
        return Err(ConfigError::Invalid(name));
    }
    Ok(value)
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "set exactly one of AEX_GATEWAY_PUBLIC_KEY_FILE, AEX_GATEWAY_PUBLIC_KEY_PEM, or AEX_GATEWAY_PUBLIC_KEY_DER_BASE64"
    )]
    PublicKeySource,
    #[error("required environment variable {0} is not set")]
    Missing(&'static str),
    #[error("environment variable {0} is invalid")]
    Invalid(&'static str),
    #[error("public key is invalid: {0}")]
    PublicKey(String),
    #[error("system DNS resolver configuration is invalid: {0}")]
    Resolver(String),
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn builtins_cover_guest_escape_destinations() {
        for address in [
            Ipv4Addr::new(10, 1, 2, 3),
            Ipv4Addr::new(127, 0, 0, 1),
            Ipv4Addr::new(169, 254, 169, 254),
            Ipv4Addr::new(172, 31, 1, 1),
            Ipv4Addr::new(192, 168, 1, 1),
            Ipv4Addr::new(224, 1, 1, 1),
        ] {
            assert!(!brain_protocol::network::is_public_unicast(&address.into()));
        }
        assert_eq!(builtin_deny_hosts(), ["aex.dev", "*.aex.dev"]);
    }
}
