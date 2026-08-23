//! Target identity and the immutable per-generation spec.

use super::*;

pub const MAX_IMAGE_IDENTITY_BYTES: usize = 256;
/// One TiB: far above any real plane shape, low enough to catch a units mistake immediately.
pub const MAX_MATERIALIZED_MIB: u64 = 1_048_576;

/// A logical target within one root session tree.
// Deliberately not serde-constructible: parse-only constructors are the sole entry points, so a
// deserialized value can never bypass the identifier grammar.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TargetKey {
    pub root_id: String,
    /// `target:environment:<binding_ref>` or `target:additional:<sandbox_id>`.
    pub target_key: String,
}

impl TargetKey {
    pub fn for_environment(
        root_id: impl Into<String>,
        binding_ref: impl Into<String>,
    ) -> Result<Self, MaterializationError> {
        let root_id = root_id.into();
        let binding_ref = binding_ref.into();
        validate_identifier(&root_id, "root_id")?;
        validate_identifier(&binding_ref, "binding_ref")?;
        Ok(Self {
            root_id,
            target_key: format!("{ENVIRONMENT_TARGET_PREFIX}{binding_ref}"),
        })
    }

    pub fn additional(
        root_id: impl Into<String>,
        sandbox_id: impl Into<String>,
    ) -> Result<Self, MaterializationError> {
        let root_id = root_id.into();
        let sandbox_id = sandbox_id.into();
        validate_identifier(&root_id, "root_id")?;
        validate_identifier(&sandbox_id, "sandbox_id")?;
        Ok(Self {
            root_id,
            target_key: format!("target:additional:{sandbox_id}"),
        })
    }

    #[must_use]
    pub fn is_environment(&self) -> bool {
        self.target_key.starts_with(ENVIRONMENT_TARGET_PREFIX)
    }

    pub fn validate(&self) -> Result<(), MaterializationError> {
        validate_identifier(&self.root_id, "root_id")?;
        let identity = self.target_identity()?;
        validate_identifier(
            identity,
            if self.is_environment() {
                "binding_ref"
            } else {
                "sandbox_id"
            },
        )
    }

    /// Environment binding or additional sandbox identity used for capability minting.
    pub fn target_identity(&self) -> Result<&str, MaterializationError> {
        if let Some(binding_ref) = self.target_key.strip_prefix(ENVIRONMENT_TARGET_PREFIX) {
            return Ok(binding_ref);
        }
        self.target_key
            .strip_prefix("target:additional:")
            .ok_or(MaterializationError::InvalidIdentity("target_key"))
    }
}

/// Everything that must remain immutable for one physical generation.
// Serialize feeds the canonical CAS digest only; Deserialize is deliberately absent so a spec
// can never enter the process without `TargetSpec::new` validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetSpec {
    pub connector: ConnectorClass,
    /// Immutable image digest/version identity resolved by the trusted Environment process.
    pub image_identity: String,
    /// Plane-owned physical size/class. Tenant input never becomes a provider identifier.
    pub resource_class: String,
    /// Physical provider memory charged against the account/region materialization quota.
    pub materialized_mib: u64,
    /// Digest of the exact execution-resource ceiling sealed for this generation.
    pub resource_policy_digest: String,
    /// Digest of the exact network ceiling sealed for this generation. Connector class alone is
    /// insufficient because two allowlists can name different destinations.
    pub network_policy_digest: String,
}

impl TargetSpec {
    pub fn new(
        connector: ConnectorClass,
        image_identity: impl Into<String>,
        resource_class: impl Into<String>,
        materialized_mib: u64,
        resource_policy_digest: impl Into<String>,
        network_policy_digest: impl Into<String>,
    ) -> Result<Self, MaterializationError> {
        let image_identity = image_identity.into();
        let resource_class = resource_class.into();
        let resource_policy_digest = resource_policy_digest.into();
        let network_policy_digest = network_policy_digest.into();
        validate_bounded_token(&image_identity, "image_identity", MAX_IMAGE_IDENTITY_BYTES)?;
        validate_identifier(&resource_class, "resource_class")?;
        if materialized_mib == 0 || materialized_mib > MAX_MATERIALIZED_MIB {
            return Err(MaterializationError::InvalidCapacity);
        }
        validate_digest(&resource_policy_digest, "resource_policy_digest")?;
        validate_digest(&network_policy_digest, "network_policy_digest")?;
        Ok(Self {
            connector,
            image_identity,
            resource_class,
            materialized_mib,
            resource_policy_digest,
            network_policy_digest,
        })
    }

    /// Stable digest used by storage CAS expressions. This is an internal identity, not a fork of
    /// Brain's public contract digest. Canonical JSON (JCS) keeps the digest independent of
    /// struct field order.
    #[must_use]
    pub fn digest(&self) -> String {
        let encoded = serde_jcs::to_vec(self).expect("TargetSpec serialization is infallible");
        hex::encode(Sha256::digest(encoded))
    }
}
