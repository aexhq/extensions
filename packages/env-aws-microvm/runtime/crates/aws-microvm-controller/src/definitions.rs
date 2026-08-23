//! Durable, non-secret binding and preparation definitions.
//!
//! These rows make an opaque Environment-issued `binding_ref` recoverable after a Environment process crash.
//! They share the plane-local registry table with target rows, but use disjoint sort-key prefixes.
//! Short-lived bundle-fetch authorities and environment values are deliberately excluded: callers
//! may only supply the immutable public projection needed to reconstruct routing and bundles.

use std::collections::HashMap;

use crate::dynamo::{conditional_failure, n, s};
use aws_sdk_dynamodb::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_dynamodb::types::AttributeValue;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest as _, Sha256};

const ROOT_ID: &str = "root_id";
const TARGET_KEY: &str = "target_key";
const RECORD_TYPE: &str = "record_type";
const RECORD_DIGEST: &str = "record_digest";
const PUBLIC_PAYLOAD: &str = "public_payload";
const UPDATED_AT_MS: &str = "updated_at_ms";
const MAX_PUBLIC_PAYLOAD_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionKind {
    Binding,
    Preparation,
    RootSeal,
}

impl DefinitionKind {
    const fn prefix(self) -> &'static str {
        match self {
            Self::Binding => "binding:",
            Self::Preparation => "preparation:",
            Self::RootSeal => "root-seal:",
        }
    }

    const fn record_type(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::Preparation => "preparation",
            Self::RootSeal => "root_seal",
        }
    }

    fn parse(value: &str) -> Result<Self, DefinitionError> {
        match value {
            "binding" => Ok(Self::Binding),
            "preparation" => Ok(Self::Preparation),
            "root_seal" => Ok(Self::RootSeal),
            _ => Err(DefinitionError::Corrupt("unknown definition type".into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionRecord {
    pub root_id: String,
    pub kind: DefinitionKind,
    pub definition_id: String,
    pub record_digest: String,
    /// Canonical JSON containing only immutable, non-secret contract data.
    pub public_payload: Vec<u8>,
    pub updated_at_ms: u64,
}

impl DefinitionRecord {
    pub fn canonical(
        root_id: impl Into<String>,
        kind: DefinitionKind,
        definition_id: impl Into<String>,
        public_projection: &impl Serialize,
        updated_at_ms: u64,
    ) -> Result<Self, DefinitionError> {
        let root_id = root_id.into();
        let definition_id = definition_id.into();
        validate_identifier(&root_id, "root_id")?;
        validate_identifier(&definition_id, "definition_id")?;
        let public_payload = serde_jcs::to_vec(public_projection)
            .map_err(|error| DefinitionError::InvalidPayload(error.to_string()))?;
        if public_payload.is_empty() || public_payload.len() > MAX_PUBLIC_PAYLOAD_BYTES {
            return Err(DefinitionError::PayloadTooLarge);
        }
        let record_digest = hex::encode(Sha256::digest(&public_payload));
        Ok(Self {
            root_id,
            kind,
            definition_id,
            record_digest,
            public_payload,
            updated_at_ms,
        })
    }

    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, DefinitionError> {
        serde_json::from_slice(&self.public_payload)
            .map_err(|error| DefinitionError::Corrupt(error.to_string()))
    }

    fn sort_key(&self) -> String {
        format!("{}{}", self.kind.prefix(), self.definition_id)
    }

    fn validate(&self) -> Result<(), DefinitionError> {
        validate_identifier(&self.root_id, "root_id")?;
        validate_identifier(&self.definition_id, "definition_id")?;
        validate_digest(&self.record_digest)?;
        if self.public_payload.is_empty() || self.public_payload.len() > MAX_PUBLIC_PAYLOAD_BYTES {
            return Err(DefinitionError::PayloadTooLarge);
        }
        if hex::encode(Sha256::digest(&self.public_payload)) != self.record_digest {
            return Err(DefinitionError::Corrupt(
                "definition payload digest mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallDefinition {
    Installed,
    Existing,
}

/// Strongly consistent binding/preparation registry. Clone shares the SDK connection pool.
#[derive(Clone)]
pub struct DynamoDefinitionRegistry {
    db: aws_sdk_dynamodb::Client,
    table: String,
}

impl DynamoDefinitionRegistry {
    pub fn new(db: aws_sdk_dynamodb::Client, table: impl Into<String>) -> Self {
        Self {
            db,
            table: table.into(),
        }
    }

    pub async fn install(
        &self,
        record: &DefinitionRecord,
    ) -> Result<InstallDefinition, DefinitionError> {
        record.validate()?;
        let result = self
            .db
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item(record)))
            .condition_expression("attribute_not_exists(root_id)")
            .send()
            .await;
        match result {
            Ok(_) => Ok(InstallDefinition::Installed),
            Err(error) if conditional_failure(&error) => {
                let existing = self
                    .get(record.root_id.as_str(), record.kind, &record.definition_id)
                    .await?;
                match existing {
                    Some(existing) if existing.record_digest == record.record_digest => {
                        Ok(InstallDefinition::Existing)
                    }
                    Some(_) => Err(DefinitionError::Conflict),
                    None => Err(DefinitionError::Storage(
                        "definition changed during conditional installation".into(),
                    )),
                }
            }
            Err(error) => Err(storage_error("install definition", &error)),
        }
    }

    pub async fn get(
        &self,
        root_id: &str,
        kind: DefinitionKind,
        definition_id: &str,
    ) -> Result<Option<DefinitionRecord>, DefinitionError> {
        validate_identifier(root_id, "root_id")?;
        validate_identifier(definition_id, "definition_id")?;
        let output = self
            .db
            .get_item()
            .table_name(&self.table)
            .key(ROOT_ID, s(root_id))
            .key(TARGET_KEY, s(format!("{}{definition_id}", kind.prefix())))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| storage_error("get definition", &error))?;
        output.item().map(parse_record).transpose()
    }

    /// Idempotently removes a bounded page of binding/preparation definitions for one deleted
    /// root. Target and capacity rows share the table and are deliberately outside this method's
    /// authority. Returns `true` only after both definition prefixes are confirmed empty.
    pub async fn purge_root_page(
        &self,
        root_id: &str,
        limit: usize,
    ) -> Result<bool, DefinitionError> {
        validate_identifier(root_id, "root_id")?;
        if !(1..=25).contains(&limit) {
            return Err(DefinitionError::InvalidLimit);
        }
        for prefix in [
            DefinitionKind::Binding.prefix(),
            DefinitionKind::Preparation.prefix(),
            DefinitionKind::RootSeal.prefix(),
        ] {
            let output = self
                .db
                .query()
                .table_name(&self.table)
                .key_condition_expression("root_id = :root_id AND begins_with(target_key, :prefix)")
                .expression_attribute_values(":root_id", s(root_id))
                .expression_attribute_values(":prefix", s(prefix))
                .consistent_read(true)
                .limit(limit as i32)
                .send()
                .await
                .map_err(|error| storage_error("list root definitions", &error))?;
            let keys = output
                .items()
                .iter()
                .map(|item| {
                    item.get(TARGET_KEY)
                        .and_then(|value| value.as_s().ok())
                        .cloned()
                        .ok_or_else(|| {
                            DefinitionError::Corrupt("definition query returned no sort key".into())
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if !keys.is_empty() {
                futures_util::future::try_join_all(keys.into_iter().map(|key| async move {
                    self.db
                        .delete_item()
                        .table_name(&self.table)
                        .key(ROOT_ID, s(root_id))
                        .key(TARGET_KEY, s(key))
                        .send()
                        .await
                        .map_err(|error| storage_error("delete root definition", &error))?;
                    Ok::<(), DefinitionError>(())
                }))
                .await?;
                // A subsequent invocation confirms absence instead of inferring it from a
                // possibly full page. This makes partial concurrent deletes safely retryable.
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn item(record: &DefinitionRecord) -> HashMap<String, AttributeValue> {
    HashMap::from([
        (ROOT_ID.into(), s(&record.root_id)),
        (TARGET_KEY.into(), s(record.sort_key())),
        (RECORD_TYPE.into(), s(record.kind.record_type())),
        (RECORD_DIGEST.into(), s(&record.record_digest)),
        (
            PUBLIC_PAYLOAD.into(),
            s(String::from_utf8(record.public_payload.clone()).expect("JCS JSON is UTF-8")),
        ),
        (UPDATED_AT_MS.into(), n(record.updated_at_ms)),
    ])
}

fn parse_record(
    attrs: &HashMap<String, AttributeValue>,
) -> Result<DefinitionRecord, DefinitionError> {
    let string = |name: &'static str| -> Result<String, DefinitionError> {
        attrs
            .get(name)
            .and_then(|value| value.as_s().ok())
            .cloned()
            .ok_or_else(|| DefinitionError::Corrupt(format!("missing string {name}")))
    };
    let number = |name: &'static str| -> Result<u64, DefinitionError> {
        attrs
            .get(name)
            .and_then(|value| value.as_n().ok())
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| DefinitionError::Corrupt(format!("missing numeric {name}")))
    };
    let root_id = string(ROOT_ID)?;
    let kind = DefinitionKind::parse(&string(RECORD_TYPE)?)?;
    let sort_key = string(TARGET_KEY)?;
    let definition_id = sort_key
        .strip_prefix(kind.prefix())
        .ok_or_else(|| DefinitionError::Corrupt("definition sort key/type mismatch".into()))?
        .to_owned();
    let record = DefinitionRecord {
        root_id,
        kind,
        definition_id,
        record_digest: string(RECORD_DIGEST)?,
        public_payload: string(PUBLIC_PAYLOAD)?.into_bytes(),
        updated_at_ms: number(UPDATED_AT_MS)?,
    };
    record.validate()?;
    Ok(record)
}

fn validate_identifier(value: &str, field: &'static str) -> Result<(), DefinitionError> {
    environment_policy::identity::validate_identifier(value, field)
        .map_err(|error| DefinitionError::InvalidIdentity(error.field))
}

fn validate_digest(value: &str) -> Result<(), DefinitionError> {
    environment_policy::identity::validate_digest(value, "digest")
        .map_err(|_| DefinitionError::Corrupt("definition digest is invalid".into()))
}

fn storage_error<E: ProvideErrorMetadata, R>(
    operation: &str,
    error: &SdkError<E, R>,
) -> DefinitionError {
    DefinitionError::Storage(crate::dynamo::storage_detail(operation, error))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DefinitionError {
    #[error("{0} does not satisfy the canonical Environment identifier grammar")]
    InvalidIdentity(&'static str),
    #[error("definition public projection is invalid: {0}")]
    InvalidPayload(String),
    #[error("definition public projection exceeds the registry item bound")]
    PayloadTooLarge,
    #[error("opaque definition identity is already sealed to different data")]
    Conflict,
    #[error("definition purge page limit must be between 1 and 25")]
    InvalidLimit,
    #[error("definition registry is unavailable: {0}")]
    Storage(String),
    #[error("definition registry contains an invalid record: {0}")]
    Corrupt(String),
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Projection {
        capability: String,
        required_env: Vec<String>,
    }

    #[test]
    fn canonical_rows_round_trip_and_do_not_contain_authorities_or_values() {
        let projection = Projection {
            capability: "tool.run".into(),
            required_env: vec!["API_KEY".into()],
        };
        let record = DefinitionRecord::canonical(
            "root-1",
            DefinitionKind::Binding,
            "binding-1",
            &projection,
            42,
        )
        .unwrap();
        let row = item(&record);
        let parsed = parse_record(&row).unwrap();
        assert_eq!(parsed.decode::<Projection>().unwrap(), projection);
        let encoded = String::from_utf8(parsed.public_payload).unwrap();
        assert!(encoded.contains("API_KEY"));
        for forbidden in ["secret-value", "https://fetch.example", "authorization"] {
            assert!(!encoded.contains(forbidden));
            assert!(!row.contains_key(forbidden));
        }
    }

    #[test]
    fn canonicalization_makes_object_key_order_idempotent() {
        let left = serde_json::json!({"z": 1, "a": 2});
        let right = serde_json::json!({"a": 2, "z": 1});
        let left = DefinitionRecord::canonical(
            "root-1",
            DefinitionKind::Preparation,
            "session-1",
            &left,
            1,
        )
        .unwrap();
        let right = DefinitionRecord::canonical(
            "root-1",
            DefinitionKind::Preparation,
            "session-1",
            &right,
            2,
        )
        .unwrap();
        assert_eq!(left.record_digest, right.record_digest);
        assert_eq!(left.public_payload, right.public_payload);
    }

    #[test]
    fn internal_root_seal_namespace_cannot_collide_with_a_session_id() {
        let projection = serde_json::json!({"root_id": "root-1"});
        let session = DefinitionRecord::canonical(
            "root-1",
            DefinitionKind::Preparation,
            "physical",
            &projection,
            1,
        )
        .unwrap();
        let root = DefinitionRecord::canonical(
            "root-1",
            DefinitionKind::RootSeal,
            "physical",
            &projection,
            1,
        )
        .unwrap();
        assert_ne!(session.sort_key(), root.sort_key());
    }
}
