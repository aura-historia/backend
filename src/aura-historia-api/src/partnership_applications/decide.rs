use super::{
    types::{
        AdminPartnershipApplicationData, DecidePartnershipApplicationData,
        PartnershipApplicationDecisionData,
    },
    util::{parse_id, parse_json},
};
use crate::{auth::protected_context, error::ApiError, state::PartnershipApplicationsState};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use partnership_service::use_cases::commands::{
    approve_partnership_application::ApprovePartnershipApplicationCommand,
    reject_partnership_application::RejectPartnershipApplicationCommand,
};

pub(super) async fn decide(
    State(state): State<PartnershipApplicationsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let application_id = match parse_id(&raw_id) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let request: DecidePartnershipApplicationData = match parse_json(&body) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };

    match request.decision {
        PartnershipApplicationDecisionData::Approve => match state
            .approve
            .execute(
                &context,
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await
        {
            Ok(result) => {
                Json(AdminPartnershipApplicationData::from(result.application)).into_response()
            }
            Err(error) => ApiError::from(error).into_response(),
        },
        PartnershipApplicationDecisionData::Reject => match state
            .reject
            .execute(
                &context,
                RejectPartnershipApplicationCommand { application_id },
            )
            .await
        {
            Ok(result) => {
                Json(AdminPartnershipApplicationData::from(result.application)).into_response()
            }
            Err(error) => ApiError::from(error).into_response(),
        },
    }
}
