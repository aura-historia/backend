use super::util::parse_id;
use crate::{auth::protected_context, error::ApiError, state::PartnershipApplicationsState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use partnership_service::use_cases::commands::withdraw_partnership_application::WithdrawPartnershipApplicationCommand;

pub(super) async fn withdraw(
    State(state): State<PartnershipApplicationsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let application_id = match parse_id(&raw_id) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };

    match state
        .withdraw
        .execute(
            &context,
            WithdrawPartnershipApplicationCommand { application_id },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}
