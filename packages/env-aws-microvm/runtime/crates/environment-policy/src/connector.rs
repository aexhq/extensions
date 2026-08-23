//! The managed-sandbox connector vocabulary. Catalog resolution and provider references live in
//! `environment-core::connector`; this is only the class names sealed into payloads and specs.

use serde::{Deserialize, Serialize};

/// The only managed-sandbox connector classes in the MVP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorClass {
    None,
    Public,
    Allowlist,
}
