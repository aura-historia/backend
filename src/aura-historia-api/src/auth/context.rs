use crate::auth::{ProtectedAuthExtractor, RequestMetadata, TransportPrincipal};
use crate::error::{ApiError, FORBIDDEN};
use crate::transport::{CORRELATION_ID_HEADER, REQUEST_ID_HEADER};
use application::operation_context::OperationContext;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use user_core::user_id::UserId;

pub async fn protected_context(
    authenticator: &dyn crate::auth::TokenAuthenticator,
    headers: &HeaderMap,
) -> Result<(OperationContext, UserId), Box<axum::response::Response>> {
    let metadata = request_metadata(headers);
    let principal = ProtectedAuthExtractor::new(authenticator)
        .extract(headers, &metadata)
        .await
        .map_err(|error| Box::new(ApiError::from(error).into_response()))?;
    let user_id = user_id(&principal).ok_or_else(|| {
        Box::new(
            ApiError::forbidden(FORBIDDEN)
                .with_detail("User principal is required.")
                .into_response(),
        )
    })?;
    Ok((principal.operation_context(metadata), user_id))
}

fn user_id(principal: &TransportPrincipal) -> Option<UserId> {
    match principal {
        TransportPrincipal::Anonymous => None,
        TransportPrincipal::User { user_id, .. } => Some(*user_id),
    }
}

pub fn request_metadata(headers: &HeaderMap) -> RequestMetadata {
    let request_id = headers
        .get(&REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("missing-request-id");
    let correlation_id = headers
        .get(&CORRELATION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or(request_id);
    RequestMetadata::new(request_id, correlation_id)
}
