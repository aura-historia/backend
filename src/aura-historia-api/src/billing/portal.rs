use super::types::BillingSessionData;
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::BillingState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use billing_service::use_cases::CreateBillingPortalSessionCommand;

pub(crate) async fn portal(State(state): State<BillingState>, headers: HeaderMap) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state
        .portal
        .execute(
            &context,
            CreateBillingPortalSessionCommand {
                idempotency_key: None,
            },
        )
        .await
    {
        Ok(result) => (
            StatusCode::CREATED,
            Json(BillingSessionData { url: result.url }),
        )
            .into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}
