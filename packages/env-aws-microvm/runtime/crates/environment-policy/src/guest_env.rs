//! Sandbox environment policy applied at both trusted-adapter and guest ingress.

use zeroize::Zeroize as _;

const MAX_SECRET_NAMES: usize = 128;
const MAX_SECRET_NAME_BYTES: usize = 128;
const MAX_SECRET_VALUE_UTF8_BYTES: usize = 8 * 1024;
const MAX_SECRET_DOCUMENT_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretMaterialError {
    #[error("secret document declares more names than Brain's custody bound")]
    TooManyNames,
    #[error("secret values do not match the declared names one-to-one")]
    NamesValuesMismatch,
    #[error("a declared secret name is not a valid environment variable name")]
    InvalidName,
    #[error("a secret value exceeds its byte bound or contains a NUL byte")]
    InvalidValue,
    #[error("the canonical secret document exceeds Brain's custody byte bound")]
    DocumentTooLarge,
}

#[must_use]
pub fn environment_name_is_valid(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_SECRET_NAME_BYTES
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

/// Names an untrusted Tool must never receive: loader/interpreter injection vectors, proxy
/// overrides, and the Environment's own namespace. Extending this list is a policy change here, not a
/// per-caller patch.
#[must_use]
pub fn reserved_tool_environment(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.starts_with("LD_")
        || name.starts_with("ENVIRONMENT_")
        || name.starts_with("AEX_")
        || matches!(
            name.as_str(),
            "PATH"
                | "HOME"
                | "USER"
                | "LOGNAME"
                | "LANG"
                | "TERM"
                | "CI"
                | "IFS"
                | "ENV"
                | "BASH_ENV"
                | "SHELLOPTS"
                | "PS4"
                | "NODE_OPTIONS"
                | "NODE_PATH"
                | "NODE_REPL_EXTERNAL_MODULE"
                | "HTTP_PROXY"
                | "HTTPS_PROXY"
                | "ALL_PROXY"
                | "NO_PROXY"
                | "GIT_TERMINAL_PROMPT"
                | "CARGO_HOME"
                | "RUSTUP_HOME"
                | "GOPATH"
                | "GOMODCACHE"
                | "NPM_CONFIG_PREFIX"
                | "NPM_CONFIG_CACHE"
                | "PIP_CACHE_DIR"
                | "PIPX_HOME"
                | "PIPX_BIN_DIR"
                | "UV_CACHE_DIR"
                | "GCONV_PATH"
                | "LOCPATH"
                | "NLSPATH"
                | "OPENSSL_CONF"
                | "OPENSSL_MODULES"
                | "SSLKEYLOGFILE"
        )
}

/// Applies Brain's exact custody-document boundary again at both trusted-adapter and guest
/// ingress, reporting which rule refused the document. The temporary canonical encoding is
/// immediately zeroized; callers separately zeroize the owned strings after delivery.
pub fn secret_material_fits(
    env_names: &[String],
    values: &std::collections::HashMap<String, String>,
) -> Result<(), SecretMaterialError> {
    if env_names.len() > MAX_SECRET_NAMES {
        return Err(SecretMaterialError::TooManyNames);
    }
    if values.len() != env_names.len() {
        return Err(SecretMaterialError::NamesValuesMismatch);
    }
    for (index, name) in env_names.iter().enumerate() {
        if !environment_name_is_valid(name) {
            return Err(SecretMaterialError::InvalidName);
        }
        if env_names[..index].iter().any(|prior| prior == name) || !values.contains_key(name) {
            return Err(SecretMaterialError::NamesValuesMismatch);
        }
    }
    if values
        .values()
        .any(|value| value.len() > MAX_SECRET_VALUE_UTF8_BYTES || value.contains('\0'))
    {
        return Err(SecretMaterialError::InvalidValue);
    }
    let Ok(mut canonical) = serde_jcs::to_vec(values) else {
        return Err(SecretMaterialError::DocumentTooLarge);
    };
    let fits = canonical.len() <= MAX_SECRET_DOCUMENT_BYTES;
    canonical.zeroize();
    if fits {
        Ok(())
    } else {
        Err(SecretMaterialError::DocumentTooLarge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_document_accepts_the_brain_exact_boundary_and_rejects_plus_one() {
        let names = vec!["A".into()];
        let exact_value = format!("{}aaaaaaaa", "é".repeat(2040));
        let exact = std::collections::HashMap::from([("A".into(), exact_value)]);
        assert_eq!(
            serde_jcs::to_vec(&exact).unwrap().len(),
            MAX_SECRET_DOCUMENT_BYTES
        );
        assert_eq!(secret_material_fits(&names, &exact), Ok(()));

        let oversized_value = format!("{}aaaaaaaa€", "é".repeat(2039));
        let oversized = std::collections::HashMap::from([("A".into(), oversized_value)]);
        assert_eq!(
            serde_jcs::to_vec(&oversized).unwrap().len(),
            MAX_SECRET_DOCUMENT_BYTES + 1
        );
        assert_eq!(
            secret_material_fits(&names, &oversized),
            Err(SecretMaterialError::DocumentTooLarge)
        );
        assert!(!environment_name_is_valid("A-B"));
        assert!(!environment_name_is_valid("9TOKEN"));
    }
}
