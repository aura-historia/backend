//! Shared HTTP admission envelope and result mapping for partner ingestion verbs.
use super::types::MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE;
use crate::{
    auth::{TokenAuthenticator, protected_context},
    error::{
        ApiError, BAD_BODY_VALUE, BAD_HEADER_VALUE, FORBIDDEN, INVALID_CREDENTIALS,
        INVALID_OBJECT_ID, PRODUCT_LISTING_INTERNAL_ERROR, PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE,
    },
};
use application::operation_context::{
    CredentialAuthorizationError, CredentialCapability, OperationContext,
};
use axum::{
    Json,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_service::use_cases::{
    IndexedProductListingIngestionIntent, ProductListingIngestionIdempotencyKey,
    ProductListingIngestionIntent, ProductListingIngestionItemOutcome,
    ProductListingIngestionNotAttemptedReason, ProductListingIngestionOutcome,
    ProductListingIngestionRejectionReason, ProductListingIngestionSubmissionError,
    ProductListingIngestionSubmissionResult,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;

const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

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

pub(super) fn safe_source_id(value: &str) -> Option<String> {
    if value.len() > 128 || value.chars().any(char::is_control) {
        return None;
    }
    SourceListingId::try_from(value)
        .ok()
        .map(|id| id.to_string())
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
    ProductListingIngestionIdempotencyKey::new(value)
        .map(Some)
        .map_err(|_| ApiError::bad_request(BAD_HEADER_VALUE).with_header_field("Idempotency-Key"))
}

pub(super) fn parse_envelope(body: &str) -> Result<Vec<Box<RawValue>>, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }
    // Parse the *entire* body before any sends, retaining each item's exact JSON representation.
    // Parsing through Value would discard duplicate fields before strict DTO deserialization.
    let items: Vec<Box<RawValue>> = serde_json::from_str(body)
        .map_err(|_| ApiError::bad_request(BAD_BODY_VALUE).with_detail("Expected a JSON array."))?;
    if items.len() > MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail(format!(
            "Body cannot contain more than {MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE} products."
        )));
    }
    Ok(items)
}

pub(super) fn parse_items<T: DeserializeOwned>(
    raw_items: Vec<Box<RawValue>>,
    source_id: impl Fn(&T) -> &str,
    into_intent: impl Fn(T) -> Result<ProductListingIngestionIntent, ApiError>,
) -> (
    Vec<IndexedProductListingIngestionIntent>,
    Vec<Option<String>>,
    Vec<BatchFailure>,
) {
    let mut items = Vec::with_capacity(raw_items.len());
    let mut source_ids = vec![None; raw_items.len()];
    let mut failures = Vec::new();
    for (index, raw) in raw_items.into_iter().enumerate() {
        let product: T = match serde_json::from_str(raw.get()) {
            Ok(product) => product,
            Err(_) => {
                failures.push(BatchFailure::invalid(index, None, "BAD_BODY_VALUE"));
                continue;
            }
        };
        // Index is authoritative; echo only a short, printable, valid listing key.
        source_ids[index] = safe_source_id(source_id(&product));
        match into_intent(product) {
            Ok(intent) => items.push(IndexedProductListingIngestionIntent { index, intent }),
            Err(error) => failures.push(BatchFailure::invalid(
                index,
                source_ids[index].clone(),
                if error.code() == INVALID_OBJECT_ID {
                    "INVALID_OBJECT_ID"
                } else {
                    "BAD_BODY_VALUE"
                },
            )),
        }
    }
    (items, source_ids, failures)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FailureKind {
    Invalid,
    MessageSize,
    Send,
    Internal,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BatchFailure {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_listing_id: Option<String>,
    pub error: &'static str,
    pub retryable: bool,
    #[serde(skip)]
    pub kind: FailureKind,
}

impl BatchFailure {
    pub fn invalid(index: usize, source_listing_id: Option<String>, error: &'static str) -> Self {
        Self {
            index,
            source_listing_id,
            error,
            retryable: false,
            kind: FailureKind::Invalid,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BatchReport {
    submission_id: String,
    accepted_count: usize,
    failures: Vec<BatchFailure>,
}

pub(super) fn report(
    result: ProductListingIngestionSubmissionResult,
    mut local_failures: Vec<BatchFailure>,
    source_ids: &[Option<String>],
) -> Response {
    let mut accepted = 0;
    let mut seen = vec![false; result.original_input_count];
    for failure in &local_failures {
        if let Some(seen) = seen.get_mut(failure.index) {
            *seen = true;
        }
    }
    for item in &result.items {
        let Some(slot) = seen.get_mut(item.index) else {
            continue;
        };
        if *slot {
            continue;
        }
        *slot = true;
        if let Some(failure) = outcome_failure(item, source_ids.get(item.index).cloned().flatten())
        {
            local_failures.push(failure);
        } else {
            accepted += 1;
        }
    }
    // Inconsistent use-case results must never be reported as confirmed acceptance.
    for (index, present) in seen.iter().enumerate() {
        if !present {
            local_failures.push(BatchFailure {
                index,
                source_listing_id: source_ids.get(index).cloned().flatten(),
                error: "ENQUEUE_UNCONFIRMED",
                retryable: true,
                kind: FailureKind::Send,
            });
        }
    }
    local_failures.sort_by_key(|failure| failure.index);
    let status = status_for(accepted, &local_failures);
    let mut response = (
        status,
        Json(BatchReport {
            submission_id: result.submission_id.to_string(),
            accepted_count: accepted,
            failures: local_failures,
        }),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(result.idempotency_key.as_str()) {
        response.headers_mut().insert(IDEMPOTENCY_KEY, value);
    }
    response
}

fn outcome_failure(
    item: &ProductListingIngestionItemOutcome,
    source_listing_id: Option<String>,
) -> Option<BatchFailure> {
    use ProductListingIngestionOutcome as Outcome;
    let (error, retryable, kind) = match &item.outcome {
        Outcome::Accepted => return None,
        Outcome::Unconfirmed => ("ENQUEUE_UNCONFIRMED", true, FailureKind::Send),
        Outcome::NotAttempted { reason, retryable } => (
            match reason {
                ProductListingIngestionNotAttemptedReason::DeadlineExceeded => {
                    "ENQUEUE_NOT_ATTEMPTED"
                }
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor => {
                    "ENQUEUE_BLOCKED"
                }
            },
            *retryable,
            if *retryable {
                FailureKind::Send
            } else {
                FailureKind::Internal
            },
        ),
        Outcome::Rejected { reason, retryable } => match reason {
            ProductListingIngestionRejectionReason::Publisher { code } => match code.as_str() {
                "INVALID_MESSAGE_SIZE" => (
                    "PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE",
                    false,
                    FailureKind::MessageSize,
                ),
                "SQS_RETRYABLE_FAILURE" => ("ENQUEUE_FAILED", true, FailureKind::Send),
                "SQS_SENDER_FAILURE" | "SQS_NOT_SENT" => {
                    ("ENQUEUE_FAILED", false, FailureKind::Internal)
                }
                _ => (
                    "PRODUCT_LISTING_INGESTION_INTERNAL_ERROR",
                    false,
                    FailureKind::Internal,
                ),
            },
            _ => ("BAD_BODY_VALUE", *retryable, FailureKind::Invalid),
        },
    };
    Some(BatchFailure {
        index: item.index,
        source_listing_id,
        error,
        retryable,
        kind,
    })
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
        headers.insert(IDEMPOTENCY_KEY, HeaderValue::from_static("contains space"));
        assert!(idempotency_key(&headers).is_err());
        headers.insert(IDEMPOTENCY_KEY, HeaderValue::from_static("okay"));
        assert_eq!(idempotency_key(&headers).unwrap().unwrap().as_str(), "okay");
    }

    #[test]
    fn publisher_outcomes_have_bounded_codes_and_correct_status() {
        use product_listing_service::use_cases::{
            ProductListingIngestionCommandId, ProductListingIngestionOutcome as Outcome,
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
            let failure = outcome_failure(&make(outcome), None).unwrap();
            assert_eq!(
                (failure.index, failure.error, failure.retryable),
                (1, code, retryable)
            );
            assert_eq!(status_for(0, &[failure]), status);
        }
    }
}
