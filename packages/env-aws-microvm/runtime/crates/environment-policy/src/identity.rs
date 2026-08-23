//! The canonical Environment identifier grammar. Every crate that names roots, targets, generations,
//! operations, or digests validates against exactly these rules.

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{field} does not satisfy the canonical Environment identifier grammar")]
pub struct IdentityError {
    pub field: &'static str,
}

/// ASCII, at most 128 bytes, alphanumeric first byte, then alphanumeric or `. _ : -`.
pub fn validate_identifier(value: &str, field: &'static str) -> Result<(), IdentityError> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(IdentityError { field });
    };
    if value.len() > 128
        || !value.is_ascii()
        || !first.is_ascii_alphanumeric()
        || chars.any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '-')))
    {
        return Err(IdentityError { field });
    }
    Ok(())
}

/// Exactly 64 lowercase hex characters.
pub fn validate_digest(value: &str, field: &'static str) -> Result<(), IdentityError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(IdentityError { field });
    }
    Ok(())
}

/// Non-empty ASCII with no whitespace, at most `max` bytes.
pub fn validate_bounded_token(
    value: &str,
    field: &'static str,
    max: usize,
) -> Result<(), IdentityError> {
    if value.is_empty()
        || value.len() > max
        || !value.is_ascii()
        || value.chars().any(char::is_whitespace)
    {
        return Err(IdentityError { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grammar_accepts_namespaced_ids_and_rejects_shape_violations() {
        assert!(validate_identifier("target:additional:sb-1", "id").is_ok());
        assert!(validate_identifier("", "id").is_err());
        assert!(validate_identifier(":leading", "id").is_err());
        assert!(validate_identifier(&"a".repeat(129), "id").is_err());
        assert!(validate_identifier("space id", "id").is_err());
        assert!(validate_digest(&"a".repeat(64), "digest").is_ok());
        assert!(validate_digest(&"A".repeat(64), "digest").is_err());
        assert!(validate_digest("abc", "digest").is_err());
        assert!(validate_bounded_token("token", "t", 8).is_ok());
        assert!(validate_bounded_token("token too long", "t", 8).is_err());
    }
}
