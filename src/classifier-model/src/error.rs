use std::time::Duration;

/// Provider-neutral classification failures. Display strings intentionally omit request and
/// provider payload data so they are safe to include in operational logs.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ClassificationError {
    #[error("classification request is invalid")]
    InvalidRequest,
    #[error("classifier provider configuration is invalid")]
    InvalidConfiguration,
    #[error("classifier authentication or authorization failed")]
    Authentication,
    #[error("classifier request timed out")]
    Timeout,
    #[error("classifier provider rate limited the request")]
    RateLimited { retry_after: Option<Duration> },
    #[error("classifier provider request failed transiently")]
    Transient,
    #[error("classifier provider response is invalid")]
    InvalidResponse,
    #[error("classifier provider does not support the requested capability")]
    UnsupportedCapability,
    #[error("candidate cannot be classified permanently")]
    PermanentCandidateFailure,
}

impl ClassificationError {
    pub const fn category(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidConfiguration => "invalid_configuration",
            Self::Authentication => "authentication",
            Self::Timeout => "timeout",
            Self::RateLimited { .. } => "rate_limited",
            Self::Transient => "transient_provider_failure",
            Self::InvalidResponse => "invalid_provider_response",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::PermanentCandidateFailure => "permanent_candidate_failure",
        }
    }

    pub const fn retry_category(&self) -> &'static str {
        match self {
            Self::Timeout | Self::RateLimited { .. } | Self::Transient | Self::InvalidResponse => {
                "retryable"
            }
            Self::InvalidRequest
            | Self::InvalidConfiguration
            | Self::Authentication
            | Self::UnsupportedCapability
            | Self::PermanentCandidateFailure => "operator_or_candidate_action",
        }
    }
}
