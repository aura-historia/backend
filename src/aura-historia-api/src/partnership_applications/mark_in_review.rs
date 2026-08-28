use super::{types::AdminPartnershipApplicationData, util::parse_id};
use crate::{auth::protected_context, error::ApiError, state::PartnershipApplicationsState};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use partnership_service::use_cases::commands::mark_partnership_application_in_review::MarkPartnershipApplicationInReviewCommand;

pub(super) async fn mark_in_review(
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
        .mark_in_review
        .execute(
            &context,
            MarkPartnershipApplicationInReviewCommand { application_id },
        )
        .await
    {
        Ok(result) => {
            Json(AdminPartnershipApplicationData::from(result.application)).into_response()
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}
