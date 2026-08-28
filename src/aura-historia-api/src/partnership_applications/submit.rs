use super::{
    types::{OwnPartnershipApplicationData, SubmitPartnershipApplicationData},
    util::parse_json,
};
use crate::{auth::protected_context, error::ApiError, state::PartnershipApplicationsState};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use partnership_service::use_cases::commands::submit_partnership_application::SubmitPartnershipApplicationCommand;

pub(super) async fn submit(
    State(state): State<PartnershipApplicationsState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, applicant_user_id) =
        match protected_context(state.authenticator.as_ref(), &headers).await {
            Ok(value) => value,
            Err(response) => return *response,
        };
    let request: SubmitPartnershipApplicationData = match parse_json(&body) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let proposal: partnership_core::partnership_application::PartnershipProposal =
        match request.proposal.try_into() {
            Ok(value) => value,
            Err(error) => return error.into_response(),
        };
    let command = SubmitPartnershipApplicationCommand {
        applicant_user_id,
        proposal,
    };

    match state.submit.execute(&context, command).await {
        Ok(result) => (
            StatusCode::CREATED,
            Json(OwnPartnershipApplicationData::from(result.application)),
        )
            .into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}
