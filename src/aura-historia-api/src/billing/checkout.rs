use super::types::{BillingSessionData, BillingSessionRequestData};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::state::BillingState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use billing_service::use_cases::CreateBillingCheckoutSessionCommand;

pub(crate) async fn checkout(
    State(state): State<BillingState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let request: BillingSessionRequestData = match parse_request(&body) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    match state
        .checkout
        .execute(
            &context,
            CreateBillingCheckoutSessionCommand {
                plan: request.plan,
                cycle: request.cycle,
                idempotency_key: None,
            },
        )
        .await
    {
        Ok(result) => (StatusCode::CREATED, Json(BillingSessionData::from(result))).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_request(body: &str) -> Result<BillingSessionRequestData, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Request body is required."));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}
