//! Shared HTTP admission envelope and result mapping for partner ingestion verbs.
use crate::{
    auth::{TokenAuthenticator, protected_context},
    error::{
        ApiError, BAD_HEADER_VALUE, FORBIDDEN, INVALID_CREDENTIALS, PRODUCT_LISTING_INTERNAL_ERROR,
        PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE,
    },
};
use admission::{BatchFailure, FailureKind};
pub(super) use admission::{parse_items, safe_source_id};
use application::operation_context::{
    CredentialAuthorizationError, CredentialCapability, OperationContext,
};
use aura_historia_contracts::partner_product_listings::admission;
use axum::{
    Json,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use product_listing_service::use_cases::{
    ProductListingIngestionIdempotencyKey, ProductListingIngestionSubmissionError,
    ProductListingIngestionSubmissionResult,
};
use serde_json::value::RawValue;

const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

#[allow(clippy::result_large_err)]
pub(super) async fn authorized_context(
    authenticator: &dyn TokenAuthenticator,
    headers: &HeaderMap,
) -> Result<OperationContext, Response> {
    let (context, _) = protected_context(authenticator, headers)
        .await
        .map_err(|response| *response)?;
    context
        .require_credential_capability(CredentialCapability::ProductListingsWrite)
        .map_err(|error| {
            match error {
                CredentialAuthorizationError::AuthenticationRequired(_) => {
                    ApiError::unauthorized(INVALID_CREDENTIALS)
                }
                CredentialAuthorizationError::InsufficientCapability { .. } => {
                    ApiError::forbidden(FORBIDDEN)
                }
            }
            .into_response()
        })?;
    Ok(context)
}

pub(super) fn submission_error(error: ProductListingIngestionSubmissionError) -> Response {
    match error {
        ProductListingIngestionSubmissionError::AuthenticationRequired => {
            ApiError::unauthorized(INVALID_CREDENTIALS)
        }
        ProductListingIngestionSubmissionError::Forbidden => ApiError::forbidden(FORBIDDEN),
        ProductListingIngestionSubmissionError::PublisherNotStarted { .. } => {
            ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
        }
        ProductListingIngestionSubmissionError::InconsistentInputIndices => {
            ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
        }
    }
    .into_response()
}

pub(super) fn idempotency_key(
    headers: &HeaderMap,
) -> Result<Option<ProductListingIngestionIdempotencyKey>, ApiError> {
    let mut values = headers.get_all(&IDEMPOTENCY_KEY).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(ApiError::bad_request(BAD_HEADER_VALUE).with_header_field("Idempotency-Key"));
    }
    let value = value.to_str().map_err(|_| {
        ApiError::bad_request(BAD_HEADER_VALUE).with_header_field("Idempotency-Key")
    })?;
    admission::parse_idempotency_key(value)
        .map(Some)
        .map_err(|_| ApiError::bad_request(BAD_HEADER_VALUE).with_header_field("Idempotency-Key"))
}

pub(super) fn parse_envelope(body: &str) -> Result<Vec<Box<RawValue>>, ApiError> {
    admission::parse_envelope(body).map_err(super::types::mutation_input_error)
}

pub(super) fn report(
    result: ProductListingIngestionSubmissionResult,
    failures: Vec<BatchFailure>,
    source_ids: &[Option<String>],
) -> Response {
    let idempotency_key = result.idempotency_key.as_str().to_owned();
    let report = admission::report(result, failures, source_ids);
    let status = status_for(report.accepted_count, &report.failures);
    let mut response = (status, Json(report)).into_response();
    if let Ok(value) = HeaderValue::from_str(&idempotency_key) {
        response.headers_mut().insert(IDEMPOTENCY_KEY, value);
    }
    response
}

pub(super) fn status_for(accepted: usize, failures: &[BatchFailure]) -> StatusCode {
    if accepted > 0 || failures.is_empty() {
        StatusCode::ACCEPTED
    } else if failures
        .iter()
        .any(|failure| failure.kind == FailureKind::Send)
    {
        StatusCode::SERVICE_UNAVAILABLE
    } else if failures
        .iter()
        .any(|failure| failure.kind == FailureKind::Internal)
    {
        StatusCode::INTERNAL_SERVER_ERROR
    } else if failures
        .iter()
        .all(|failure| failure.kind == FailureKind::MessageSize)
    {
        StatusCode::PAYLOAD_TOO_LARGE
    } else {
        StatusCode::BAD_REQUEST
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn envelope_preserves_duplicate_fields_and_rejects_late_syntax_errors() {
        let items = parse_envelope("[{\"a\":1,\"a\":2},null]").unwrap();
        assert_eq!(items[0].get(), "{\"a\":1,\"a\":2}");
        assert_eq!(items[1].get(), "null");
        for invalid in ["", "  ", "{}", "[{},", "[{},]", "[{},{}]garbage"] {
            assert!(parse_envelope(invalid).is_err(), "{invalid}");
        }
        assert!(parse_envelope(&format!("[{}]", vec!["null"; 101].join(","))).is_err());
    }
    #[test]
    fn status_precedence() {
        let failure = |kind, retryable| BatchFailure {
            index: 0,
            source_listing_id: None,
            error: "SAFE",
            retryable,
            kind,
        };
        assert_eq!(status_for(0, &[]), StatusCode::ACCEPTED);
        assert_eq!(
            status_for(1, &[failure(FailureKind::Send, true)]),
            StatusCode::ACCEPTED
        );
        assert_eq!(
            status_for(0, &[failure(FailureKind::Invalid, false)]),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(0, &[failure(FailureKind::MessageSize, false)]),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            status_for(
                0,
                &[
                    failure(FailureKind::MessageSize, false),
                    failure(FailureKind::Invalid, false)
                ]
            ),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(0, &[failure(FailureKind::Internal, false)]),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_for(
                0,
                &[
                    failure(FailureKind::Internal, false),
                    failure(FailureKind::Send, true)
                ]
            ),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn key_rejects_duplicate_headers_and_invalid_values() {
        let mut headers = HeaderMap::new();
        headers.append(IDEMPOTENCY_KEY, HeaderValue::from_static("first"));
        headers.append(IDEMPOTENCY_KEY, HeaderValue::from_static("second"));
        assert!(idempotency_key(&headers).is_err());
        headers.clear();
        for invalid in ["contains space", "first,second", "first, second", ","] {
            headers.insert(IDEMPOTENCY_KEY, HeaderValue::from_str(invalid).unwrap());
            let error = idempotency_key(&headers).unwrap_err();
            assert_eq!(error.code(), BAD_HEADER_VALUE, "{invalid}");
        }
        headers.insert(IDEMPOTENCY_KEY, HeaderValue::from_static("okay"));
        assert_eq!(idempotency_key(&headers).unwrap().unwrap().as_str(), "okay");
    }

    #[test]
    fn http_key_grammar_covers_visible_ascii_except_comma() {
        let mut headers = HeaderMap::new();
        for byte in 0x21..=0x7e {
            let value = HeaderValue::from_bytes(&[byte]).unwrap();
            headers.insert(IDEMPOTENCY_KEY, value);
            assert_eq!(
                idempotency_key(&headers).is_ok(),
                byte != b',',
                "byte {byte}"
            );
        }
        headers.insert(IDEMPOTENCY_KEY, HeaderValue::from_static(""));
        assert!(idempotency_key(&headers).is_err());
        headers.insert(
            IDEMPOTENCY_KEY,
            HeaderValue::from_str(&"a".repeat(128)).unwrap(),
        );
        assert!(idempotency_key(&headers).is_ok());
        headers.insert(
            IDEMPOTENCY_KEY,
            HeaderValue::from_str(&"a".repeat(129)).unwrap(),
        );
        assert!(idempotency_key(&headers).is_err());
    }

    #[test]
    fn publisher_outcomes_have_bounded_codes_and_correct_status() {
        use product_listing_service::use_cases::{
            ProductListingIngestionCommandId, ProductListingIngestionItemOutcome,
            ProductListingIngestionNotAttemptedReason, ProductListingIngestionOutcome as Outcome,
            ProductListingIngestionRejectionReason, ProductListingIngestionSubmissionId,
        };
        let command_id =
            ProductListingIngestionCommandId::from_wire(&format!("plic1_{}", "a".repeat(64)))
                .unwrap();
        let make = |outcome| ProductListingIngestionItemOutcome {
            index: 1,
            command_id: command_id.clone(),
            outcome,
        };
        let cases = [
            (
                Outcome::Unconfirmed,
                "ENQUEUE_UNCONFIRMED",
                true,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                Outcome::NotAttempted {
                    reason: ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
                    retryable: true,
                },
                "ENQUEUE_NOT_ATTEMPTED",
                true,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                Outcome::NotAttempted {
                    reason: ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                    retryable: false,
                },
                "ENQUEUE_BLOCKED",
                false,
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                Outcome::Rejected {
                    reason: ProductListingIngestionRejectionReason::Publisher {
                        code: "INVALID_MESSAGE_SIZE".into(),
                    },
                    retryable: false,
                },
                "PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE",
                false,
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
            (
                Outcome::Rejected {
                    reason: ProductListingIngestionRejectionReason::Publisher {
                        code: "SQS_SENDER_FAILURE".into(),
                    },
                    retryable: false,
                },
                "ENQUEUE_FAILED",
                false,
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                Outcome::Rejected {
                    reason: ProductListingIngestionRejectionReason::Publisher {
                        code: "SQS_RETRYABLE_FAILURE".into(),
                    },
                    retryable: true,
                },
                "ENQUEUE_FAILED",
                true,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                Outcome::Rejected {
                    reason: ProductListingIngestionRejectionReason::Publisher {
                        code: "INGESTION_INTERNAL_ERROR".into(),
                    },
                    retryable: false,
                },
                "PRODUCT_LISTING_INGESTION_INTERNAL_ERROR",
                false,
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (outcome, code, retryable, status) in cases {
            let result = ProductListingIngestionSubmissionResult {
                submission_id: ProductListingIngestionSubmissionId::from_wire(&format!(
                    "plis1_{}",
                    "b".repeat(64)
                ))
                .unwrap(),
                idempotency_key: ProductListingIngestionIdempotencyKey::new("fixture").unwrap(),
                original_input_count: 2,
                items: vec![make(outcome)],
            };
            let failure = admission::report(result, vec![], &[None, None])
                .failures
                .into_iter()
                .find(|failure| failure.index == 1)
                .unwrap();
            assert_eq!(
                (failure.index, failure.error, failure.retryable),
                (1, code, retryable)
            );
            assert_eq!(status_for(0, &[failure]), status);
        }
    }
}
