//! Signed destination capabilities and a bounded HTTP CONNECT gateway.
//!
//! The gateway is deliberately independent of Brain and AWS. A trusted Environment signs one capability
//! per sandbox generation with a plane-local KMS P-256 key. This process receives only the public
//! key, validates destinations locally, and forwards end-to-end TLS bytes without interception.

mod capability;
mod config;
mod network;
mod policy;
mod proxy;
mod tls;

pub use capability::{
    Capability, CapabilityDestination, CapabilityError, DestinationProtocol,
    MAX_CAPABILITY_PAYLOAD_BYTES, MAX_DESTINATIONS, MAX_ENCODED_TOKEN_BYTES,
    MAX_GENERATION_LIFETIME_MS, VerifiedCapability, encode_signed_token, unsigned_capability_bytes,
    verify_token,
};
pub use config::{Config, ConfigError};
pub use policy::{AuthorizedTarget, PolicyError};
pub use proxy::serve;
