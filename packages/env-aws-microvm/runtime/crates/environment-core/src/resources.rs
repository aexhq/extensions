//! Fail-closed validation for execution ceilings a selected target can actually enforce.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceRequest {
    pub timeout_ms: u64,
    pub max_output_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceSupport {
    pub max_timeout_ms: u64,
    pub max_output_bytes: u64,
}

impl ResourceSupport {
    pub fn validate(self, request: ResourceRequest) -> Result<(), ResourceError> {
        if request.timeout_ms == 0 || request.timeout_ms > self.max_timeout_ms {
            return Err(ResourceError::TimeoutUnsupported);
        }
        if request.max_output_bytes == 0 || request.max_output_bytes > self.max_output_bytes {
            return Err(ResourceError::OutputUnsupported);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ResourceError {
    #[error("the selected target cannot enforce the requested wall-clock ceiling")]
    TimeoutUnsupported,
    #[error("the selected target cannot retain the requested output ceiling")]
    OutputUnsupported,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforceable_dimensions_are_bounded() {
        let support = ResourceSupport {
            max_timeout_ms: 60_000,
            max_output_bytes: 1024,
        };
        let base = ResourceRequest {
            timeout_ms: 1_000,
            max_output_bytes: 100,
        };
        assert_eq!(support.validate(base), Ok(()));
        assert_eq!(
            support.validate(ResourceRequest {
                timeout_ms: 60_001,
                ..base
            }),
            Err(ResourceError::TimeoutUnsupported)
        );
        assert_eq!(
            support.validate(ResourceRequest {
                max_output_bytes: 1025,
                ..base
            }),
            Err(ResourceError::OutputUnsupported)
        );
    }
}
