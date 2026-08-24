use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ipnet::Ipv4Net;
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::policy::ValidatedPolicy;

/// Maximum canonical JSON bytes. With a worst-case 72-byte P-256 DER signature this encodes to
/// exactly at most [`MAX_ENCODED_TOKEN_BYTES`].
pub const MAX_CAPABILITY_PAYLOAD_BYTES: usize = 7_607;
/// One transport-owned capability bound shared by minting, verification, Bearer and Basic auth.
/// 10 KiB leaves more than 2 KiB for the CONNECT request line and normal headers after Basic's
/// second base64 expansion under the gateway's 16 KiB whole-header bound.
pub const MAX_ENCODED_TOKEN_BYTES: usize = 10 * 1024;
const MAX_DER_SIGNATURE_BYTES: usize = 72;
pub const MAX_DESTINATIONS: usize = 128;
/// `CapabilityError::Invalid` carries only static text, so the bound's message lives beside the
/// bound it names.
const MAX_DESTINATIONS_MESSAGE: &str = "destinations must contain between 1 and 128 entries";
pub const MAX_GENERATION_LIFETIME_MS: u64 = 8 * 60 * 60 * 1000;
const CLOCK_SKEW_MS: u64 = 60_000;

/// The exact payload signed by the trusted Environment control role once per sandbox generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub root_id: String,
    pub session_id: String,
    pub sandbox_id: String,
    pub generation: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    /// Brain's sealed policy identity. The gateway treats it as opaque signed context.
    pub policy_digest: String,
    pub destinations: Vec<CapabilityDestination>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDestination {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cidr: Option<Ipv4Net>,
    pub ports: Vec<u16>,
    pub protocol: DestinationProtocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationProtocol {
    Tls,
    Tcp,
}

/// A signature-checked and semantically validated capability.
#[derive(Debug, Clone)]
pub struct VerifiedCapability {
    pub capability: Capability,
    pub(crate) policy: ValidatedPolicy,
}

/// Deterministic bytes SHA-256 hashed before KMS `Sign(MessageType=DIGEST, ECDSA_SHA_256)`.
pub fn unsigned_capability_bytes(capability: &Capability) -> Result<Vec<u8>, CapabilityError> {
    validate_identity(&capability.root_id, "root_id")?;
    validate_identity(&capability.session_id, "session_id")?;
    validate_identity(&capability.sandbox_id, "sandbox_id")?;
    validate_identity(&capability.generation, "generation")?;
    validate_digest(&capability.policy_digest, "policy_digest")?;
    if capability.expires_at_ms <= capability.issued_at_ms
        || capability.expires_at_ms - capability.issued_at_ms > MAX_GENERATION_LIFETIME_MS
    {
        return Err(CapabilityError::Lifetime);
    }
    if capability.destinations.is_empty() || capability.destinations.len() > MAX_DESTINATIONS {
        return Err(CapabilityError::Invalid(MAX_DESTINATIONS_MESSAGE));
    }
    let mut canonical = capability.clone();
    for destination in &mut canonical.destinations {
        if let Some(host) = &destination.host {
            destination.host = Some(crate::policy::normalize_host_pattern(host)?);
        }
        destination.ports.sort_unstable();
    }
    canonical.destinations.sort_by(|left, right| {
        let left = serde_json::to_string(left).expect("capability destination serializes");
        let right = serde_json::to_string(right).expect("capability destination serializes");
        left.cmp(&right)
    });
    let before = canonical.destinations.len();
    canonical.destinations.dedup();
    if canonical.destinations.len() != before {
        return Err(CapabilityError::Invalid("duplicate destination"));
    }
    ValidatedPolicy::new(&canonical.destinations)?;
    let bytes = serde_json::to_vec(&canonical).map_err(CapabilityError::Json)?;
    if bytes.len() > MAX_CAPABILITY_PAYLOAD_BYTES {
        return Err(CapabilityError::TooLarge);
    }
    Ok(bytes)
}

/// Combines exact payload bytes and the DER-encoded ECDSA signature returned by AWS KMS.
pub fn encode_signed_token(
    payload: &[u8],
    der_signature: &[u8],
) -> Result<String, CapabilityError> {
    if payload.is_empty()
        || payload.len() > MAX_CAPABILITY_PAYLOAD_BYTES
        || der_signature.len() > MAX_DER_SIGNATURE_BYTES
    {
        return Err(CapabilityError::TooLarge);
    }
    Signature::from_der(der_signature).map_err(|_| CapabilityError::MalformedSignature)?;
    let token = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(der_signature)
    );
    if token.len() > MAX_ENCODED_TOKEN_BYTES {
        return Err(CapabilityError::TooLarge);
    }
    Ok(token)
}

pub fn verify_token(
    token: &str,
    public_key: &VerifyingKey,
    now_ms: u64,
) -> Result<VerifiedCapability, CapabilityError> {
    if token.len() > MAX_ENCODED_TOKEN_BYTES || token.chars().any(char::is_whitespace) {
        return Err(CapabilityError::TooLarge);
    }
    let (payload, signature) = token
        .split_once('.')
        .ok_or(CapabilityError::MalformedToken)?;
    if signature.contains('.') {
        return Err(CapabilityError::MalformedToken);
    }
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| CapabilityError::MalformedToken)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| CapabilityError::MalformedToken)?;
    if payload.is_empty()
        || payload.len() > MAX_CAPABILITY_PAYLOAD_BYTES
        || signature.len() > MAX_DER_SIGNATURE_BYTES
    {
        return Err(CapabilityError::TooLarge);
    }
    let signature =
        Signature::from_der(&signature).map_err(|_| CapabilityError::MalformedSignature)?;
    public_key
        .verify(&payload, &signature)
        .map_err(|_| CapabilityError::BadSignature)?;
    let capability: Capability = serde_json::from_slice(&payload).map_err(CapabilityError::Json)?;
    // Re-serialize and compare so duplicate fields, alternative representations and unknown data
    // never gain a second semantic interpretation after signature verification.
    if unsigned_capability_bytes(&capability)? != payload {
        return Err(CapabilityError::NonCanonical);
    }
    if capability.issued_at_ms > now_ms.saturating_add(CLOCK_SKEW_MS) {
        return Err(CapabilityError::NotYetValid);
    }
    if capability.expires_at_ms <= now_ms {
        return Err(CapabilityError::Expired);
    }
    // Mint-time canonicalization already enforces the lifetime. Keep the explicit verifier check
    // so this invariant remains local even if token decoding is refactored later.
    if capability.expires_at_ms <= capability.issued_at_ms
        || capability.expires_at_ms - capability.issued_at_ms > MAX_GENERATION_LIFETIME_MS
    {
        return Err(CapabilityError::Lifetime);
    }
    let policy = ValidatedPolicy::new(&capability.destinations)?;
    Ok(VerifiedCapability { capability, policy })
}

#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn validate_identity(value: &str, field: &'static str) -> Result<(), CapabilityError> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(CapabilityError::Invalid(field));
    };
    if value.len() > 128
        || !value.is_ascii()
        || !first.is_ascii_alphanumeric()
        || chars.any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '-')))
    {
        return Err(CapabilityError::Invalid(field));
    }
    Ok(())
}

fn validate_digest(value: &str, field: &'static str) -> Result<(), CapabilityError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CapabilityError::Invalid(field));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("capability is too large")]
    TooLarge,
    #[error("capability token is malformed")]
    MalformedToken,
    #[error("capability signature is malformed")]
    MalformedSignature,
    #[error("capability signature is invalid")]
    BadSignature,
    #[error("capability JSON is invalid: {0}")]
    Json(serde_json::Error),
    #[error("capability is not in canonical encoding")]
    NonCanonical,
    #[error("capability is not valid yet")]
    NotYetValid,
    #[error("capability expired")]
    Expired,
    #[error("capability lifetime exceeds the sandbox generation wall")]
    Lifetime,
    #[error("invalid capability field: {0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Policy(#[from] crate::policy::PolicyError),
}

#[cfg(test)]
mod tests {
    use p256::ecdsa::signature::Signer as _;
    use p256::ecdsa::{Signature, SigningKey};

    use super::*;

    fn capability(now: u64) -> Capability {
        Capability {
            root_id: "root-1".into(),
            session_id: "session-1".into(),
            sandbox_id: "default".into(),
            generation: "generation-1".into(),
            issued_at_ms: now,
            expires_at_ms: now + 60_000,
            policy_digest: "a".repeat(64),
            destinations: vec![CapabilityDestination {
                host: Some("example.com".into()),
                cidr: None,
                ports: vec![443],
                protocol: DestinationProtocol::Tls,
            }],
        }
    }

    fn signed(capability: &Capability, key: &SigningKey) -> String {
        let payload = unsigned_capability_bytes(capability).unwrap();
        let signature: Signature = key.sign(&payload);
        encode_signed_token(&payload, signature.to_der().as_bytes()).unwrap()
    }

    #[test]
    fn exact_kms_style_der_signature_verifies() {
        let now = 1_000_000;
        let key = SigningKey::from_slice(&[7; 32]).unwrap();
        let verified = verify_token(&signed(&capability(now), &key), key.verifying_key(), now)
            .expect("valid token");
        assert_eq!(verified.capability.generation, "generation-1");
    }

    #[test]
    fn mutation_expiry_and_excess_lifetime_fail_closed() {
        let now = 1_000_000;
        let key = SigningKey::from_slice(&[7; 32]).unwrap();
        let token = signed(&capability(now), &key);
        let mut bytes = token.into_bytes();
        bytes[3] = if bytes[3] == b'A' { b'B' } else { b'A' };
        assert!(matches!(
            verify_token(
                std::str::from_utf8(&bytes).unwrap(),
                key.verifying_key(),
                now
            ),
            Err(CapabilityError::BadSignature | CapabilityError::Json(_))
        ));
        assert!(matches!(
            verify_token(
                &signed(&capability(now), &key),
                key.verifying_key(),
                now + 60_000
            ),
            Err(CapabilityError::Expired)
        ));
        let mut long = capability(now);
        long.expires_at_ms = now + MAX_GENERATION_LIFETIME_MS + 1;
        assert!(matches!(
            unsigned_capability_bytes(&long),
            Err(CapabilityError::Lifetime)
        ));
        let mut empty_lifetime = capability(now);
        empty_lifetime.expires_at_ms = now;
        assert!(matches!(
            unsigned_capability_bytes(&empty_lifetime),
            Err(CapabilityError::Lifetime)
        ));
    }

    #[test]
    fn encoded_transport_bound_is_enforced_at_mint_time() {
        let key = SigningKey::from_slice(&[7; 32]).unwrap();
        let payload = vec![b'a'; MAX_CAPABILITY_PAYLOAD_BYTES];
        let signature: Signature = key.sign(&payload);
        let token = encode_signed_token(&payload, signature.to_der().as_bytes()).unwrap();
        assert!(token.len() <= MAX_ENCODED_TOKEN_BYTES);
        assert!(matches!(
            encode_signed_token(
                &[b'a'; MAX_CAPABILITY_PAYLOAD_BYTES + 1],
                signature.to_der().as_bytes()
            ),
            Err(CapabilityError::TooLarge)
        ));
    }
}
