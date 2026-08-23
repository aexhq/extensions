//! Secret-bearing newtypes: redacted `Debug`, zeroized `Drop`, parse-only construction.

use zeroize::Zeroize as _;

pub const MAX_DURABLE_LAUNCH_REQUEST_BYTES: usize = 64 * 1024;
const CONTROL_TOKEN_PREFIX: &str = "control-";
const CONTROL_TOKEN_HEX_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("generation control token is outside its exact secret boundary")]
    InvalidControlToken,
    #[error("durable provider launch request is empty or exceeds its sealed byte bound")]
    InvalidLaunchRequest,
}

/// Generation-scoped bearer for the in-guest supervisor channel. The provider JWE authenticates
/// traffic at the public provider endpoint, but an untrusted Tool shares the guest network
/// namespace and can bypass that proxy. This second bearer is delivered only in the sealed run
/// payload and retained with the installed routing row. It is never exposed through Brain's
/// public Environment contract, formatting, logs, or Tool environments.
#[derive(Clone, PartialEq, Eq)]
pub struct ControlToken(String);

impl ControlToken {
    pub fn new(value: impl Into<String>) -> Result<Self, SecretError> {
        let value = value.into();
        let Some(hex) = value.strip_prefix(CONTROL_TOKEN_PREFIX) else {
            return Err(SecretError::InvalidControlToken);
        };
        if hex.len() != CONTROL_TOKEN_HEX_BYTES * 2
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(SecretError::InvalidControlToken);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ControlToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ControlToken([redacted])")
    }
}

impl serde::Serialize for ControlToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for ControlToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

impl Drop for ControlToken {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Exact provider request retained only while a target is materializing. It can contain a
/// short-lived private-network bearer, so formatting is always redacted and dropped storage is
/// zeroized. The registry persists these bytes before provider dispatch and removes them on
/// install.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct DurableLaunchRequest(String);

impl DurableLaunchRequest {
    pub fn new(value: impl Into<String>) -> Result<Self, SecretError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_DURABLE_LAUNCH_REQUEST_BYTES {
            return Err(SecretError::InvalidLaunchRequest);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn validate(&self) -> Result<(), SecretError> {
        if self.0.is_empty() || self.0.len() > MAX_DURABLE_LAUNCH_REQUEST_BYTES {
            Err(SecretError::InvalidLaunchRequest)
        } else {
            Ok(())
        }
    }
}

impl std::fmt::Debug for DurableLaunchRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DurableLaunchRequest([redacted])")
    }
}

impl Drop for DurableLaunchRequest {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
