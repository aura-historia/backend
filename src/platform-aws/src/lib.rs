//! Bounded, loggable classification of failed AWS SDK calls, shared by every adapter that calls
//! an AWS service.

use aws_smithy_runtime_api::client::{orchestrator::HttpResponse, result::SdkError};
use aws_smithy_types::error::metadata::ProvideErrorMetadata;

/// Static classes of a failed AWS SDK call.
///
/// They come from the transport outcome, the HTTP status and the standardized AWS error code.
/// The SDK message, raw response and any unrecognized code stay with the error, which can carry
/// request details or provider text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwsSdkFailureKind {
    /// The call or its connection did not finish in time.
    Timeout,
    /// The request did not reach the service.
    Transport,
    /// The client could not build or send the request.
    Configuration,
    Throttled,
    /// The service refused the caller's identity or permissions.
    AccessDenied,
    /// The addressed resource does not exist.
    NotFound,
    /// A KMS key the service needs is disabled, missing or unusable.
    KeyUnavailable,
    /// The service failed or is unavailable.
    Unavailable,
    /// The response could not be read as the operation's output or error.
    InvalidResponse,
    /// The service rejected the request for another reason.
    Rejected,
}

impl AwsSdkFailureKind {
    /// The lower-case class used by failure diagnostics.
    pub const fn diagnostic_category(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::Configuration => "configuration",
            Self::Throttled => "throttled",
            Self::AccessDenied => "access_denied",
            Self::NotFound => "not_found",
            Self::KeyUnavailable => "kms_unavailable",
            Self::Unavailable => "unavailable",
            Self::InvalidResponse => "response_invalid",
            Self::Rejected => "rejected",
        }
    }
}

/// The class of a failed AWS SDK call and the HTTP status the service answered with, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwsSdkFailure {
    kind: AwsSdkFailureKind,
    http_status: Option<u16>,
}

const THROTTLED_CODES: &[&str] = &[
    "Throttling",
    "ThrottlingException",
    "ThrottledException",
    "RequestThrottled",
    "RequestThrottledException",
    "TooManyRequestsException",
    "RequestLimitExceeded",
    "SlowDown",
    "KmsThrottled",
];

const ACCESS_DENIED_CODES: &[&str] = &[
    "AccessDenied",
    "AccessDeniedException",
    "NotAuthorizedException",
    "UnrecognizedClientException",
    "InvalidClientTokenId",
    "InvalidSignatureException",
    "SignatureDoesNotMatch",
    "ExpiredToken",
    "ExpiredTokenException",
    "InvalidSecurity",
    "KmsAccessDenied",
];

const NOT_FOUND_CODES: &[&str] = &[
    "ResourceNotFoundException",
    "QueueDoesNotExist",
    "AWS.SimpleQueueService.NonExistentQueue",
];

const KEY_UNAVAILABLE_CODES: &[&str] = &[
    "KmsDisabled",
    "KmsInvalidState",
    "KmsNotFound",
    "KmsInvalidKeyUsage",
    "KmsOptInRequired",
    "DecryptionFailure",
];

const UNAVAILABLE_CODES: &[&str] = &[
    "InternalError",
    "InternalErrorException",
    "InternalFailure",
    "InternalServerException",
    "InternalServiceError",
    "ServiceUnavailable",
    "ServiceUnavailableException",
];

impl AwsSdkFailure {
    pub fn classify<E: ProvideErrorMetadata>(error: &SdkError<E, HttpResponse>) -> Self {
        let http_status = error
            .raw_response()
            .map(|response| response.status().as_u16());
        let kind = match error {
            SdkError::TimeoutError(_) => AwsSdkFailureKind::Timeout,
            SdkError::DispatchFailure(failure) if failure.is_timeout() => {
                AwsSdkFailureKind::Timeout
            }
            SdkError::DispatchFailure(failure) if failure.is_io() => AwsSdkFailureKind::Transport,
            SdkError::ConstructionFailure(_) | SdkError::DispatchFailure(_) => {
                AwsSdkFailureKind::Configuration
            }
            SdkError::ServiceError(failure) => service_failure(http_status, failure.err().code()),
            SdkError::ResponseError(_) => unreadable_response(http_status),
            _ => AwsSdkFailureKind::Unavailable,
        };
        Self { kind, http_status }
    }

    pub fn kind(&self) -> AwsSdkFailureKind {
        self.kind
    }

    pub fn http_status(&self) -> Option<u16> {
        self.http_status
    }
}

fn service_failure(http_status: Option<u16>, code: Option<&str>) -> AwsSdkFailureKind {
    let code_in = |codes: &[&str]| code.is_some_and(|code| codes.contains(&code));
    match http_status {
        Some(200..=299) => AwsSdkFailureKind::InvalidResponse,
        Some(429) => AwsSdkFailureKind::Throttled,
        _ if code_in(THROTTLED_CODES) => AwsSdkFailureKind::Throttled,
        Some(401 | 403) => AwsSdkFailureKind::AccessDenied,
        _ if code_in(ACCESS_DENIED_CODES) => AwsSdkFailureKind::AccessDenied,
        Some(408 | 504) => AwsSdkFailureKind::Timeout,
        _ if code_in(NOT_FOUND_CODES) => AwsSdkFailureKind::NotFound,
        _ if code_in(KEY_UNAVAILABLE_CODES) => AwsSdkFailureKind::KeyUnavailable,
        Some(500..=599) => AwsSdkFailureKind::Unavailable,
        _ if code_in(UNAVAILABLE_CODES) => AwsSdkFailureKind::Unavailable,
        _ => AwsSdkFailureKind::Rejected,
    }
}

/// A response the SDK could not parse is classified by its status alone.
fn unreadable_response(http_status: Option<u16>) -> AwsSdkFailureKind {
    match http_status {
        Some(401 | 403) => AwsSdkFailureKind::AccessDenied,
        Some(408 | 504) => AwsSdkFailureKind::Timeout,
        Some(429) => AwsSdkFailureKind::Throttled,
        Some(500..=599) => AwsSdkFailureKind::Unavailable,
        Some(400..=499) => AwsSdkFailureKind::Rejected,
        _ => AwsSdkFailureKind::InvalidResponse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::error::ErrorMetadata;

    fn response(status: u16) -> HttpResponse {
        HttpResponse::new(status.try_into().unwrap(), "SENTINEL_PROVIDER_BODY".into())
    }

    fn service_error(status: u16, code: &str) -> SdkError<ErrorMetadata, HttpResponse> {
        SdkError::service_error(
            ErrorMetadata::builder()
                .code(code)
                .message("SENTINEL_PROVIDER_MESSAGE")
                .build(),
            response(status),
        )
    }

    #[test]
    fn should_classify_service_errors_by_status_and_standardized_code() {
        use AwsSdkFailureKind as K;
        for (status, code, kind) in [
            (400, "ThrottlingException", K::Throttled),
            (400, "TooManyRequestsException", K::Throttled),
            (429, "Unrecognized", K::Throttled),
            (400, "KmsThrottled", K::Throttled),
            (403, "Unrecognized", K::AccessDenied),
            (400, "NotAuthorizedException", K::AccessDenied),
            (400, "KmsAccessDenied", K::AccessDenied),
            (400, "QueueDoesNotExist", K::NotFound),
            (400, "ResourceNotFoundException", K::NotFound),
            (400, "KmsDisabled", K::KeyUnavailable),
            (400, "DecryptionFailure", K::KeyUnavailable),
            (503, "Unrecognized", K::Unavailable),
            (400, "InternalErrorException", K::Unavailable),
            (504, "Unrecognized", K::Timeout),
            (200, "Unrecognized", K::InvalidResponse),
            (400, "ValidationException", K::Rejected),
        ] {
            let failure = AwsSdkFailure::classify(&service_error(status, code));

            assert_eq!(kind, failure.kind(), "{status} {code}");
            assert_eq!(Some(status), failure.http_status(), "{status} {code}");
        }
    }

    #[test]
    fn should_classify_an_unreadable_response_by_its_status() {
        use AwsSdkFailureKind as K;
        for (status, kind) in [
            (200, K::InvalidResponse),
            (400, K::Rejected),
            (401, K::AccessDenied),
            (408, K::Timeout),
            (429, K::Throttled),
            (502, K::Unavailable),
            (504, K::Timeout),
        ] {
            let error: SdkError<ErrorMetadata, HttpResponse> = SdkError::response_error(
                std::io::Error::other("SENTINEL_PROVIDER_BODY"),
                response(status),
            );

            let failure = AwsSdkFailure::classify(&error);

            assert_eq!(kind, failure.kind(), "{status}");
            assert_eq!(Some(status), failure.http_status());
        }
    }

    #[test]
    fn should_classify_transport_failures_without_a_status() {
        let error: SdkError<ErrorMetadata, HttpResponse> =
            SdkError::timeout_error(std::io::Error::other("SENTINEL"));

        assert_eq!(
            AwsSdkFailure {
                kind: AwsSdkFailureKind::Timeout,
                http_status: None
            },
            AwsSdkFailure::classify(&error)
        );
    }
}
