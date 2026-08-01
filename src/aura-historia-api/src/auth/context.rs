use crate::auth::{ProtectedAuthExtractor, RequestMetadata, TransportPrincipal};
use crate::error::{ApiError, FORBIDDEN};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use common::operation_context::OperationContext;
use common::user_id::UserId;

pub async fn protected_context(
    authenticator: &dyn crate::auth::TokenAuthenticator,
    headers: &HeaderMap,
) -> Result<(OperationContext, UserId), axum::response::Response> {
    let metadata = request_metadata(headers);
    let principal = ProtectedAuthExtractor::new(authenticator)
        .extract(headers, &metadata)
        .await
        .map_err(|error| ApiError::from(error).into_response())?;
    let user_id = user_id(&principal).ok_or_else(|| {
        ApiError::forbidden(FORBIDDEN)
            .with_detail("User principal is required.")
            .into_response()
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
    let request_id = uuid::Uuid::new_v4().to_string();
    let correlation_id = headers
        .get("x-correlation-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| request_id.clone());
    RequestMetadata::new(request_id, correlation_id)
}
