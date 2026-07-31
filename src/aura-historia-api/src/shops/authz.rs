use crate::auth::{ProtectedAuthExtractor, TransportPrincipal};
use crate::error::{ApiError, FORBIDDEN};
use crate::shops::shop_data::request_metadata;
use crate::state::ShopsState;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use common::operation_context::OperationContext;
use common::user_id::UserId;

pub(crate) async fn protected_context(
    state: &ShopsState,
    headers: &HeaderMap,
) -> Result<(OperationContext, UserId), axum::response::Response> {
    let metadata = request_metadata(headers);
    let principal = ProtectedAuthExtractor::new(state.authenticator.as_ref())
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
