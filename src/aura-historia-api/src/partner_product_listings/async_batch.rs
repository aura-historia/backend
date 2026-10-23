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
    let admission = admission::report(result, failures, source_ids);
    let status = status_for(admission.report.accepted_count, &admission.report.failures);
    let mut response = (status, Json(admission.report)).into_response();
    if let Ok(value) = HeaderValue::from_str(&admission.idempotency_key) {
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
#[path = "async_batch_tests.rs"]
mod tests;
