use super::{
    types::{
        AdminPartnershipApplicationData, DecidePartnershipApplicationData,
        PartnershipApplicationDecisionData,
    },
    util::{no_store, parse_id, parse_json},
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
        Err(response) => return no_store(*response),
    };
    let application_id = match parse_id(&raw_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let request: DecidePartnershipApplicationData = match parse_json(&body) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let decision = match &request.decision {
        PartnershipApplicationDecisionData::Approve => "APPROVE",
        PartnershipApplicationDecisionData::Reject => "REJECT",
    };
    let actor_id = context.principal.actor_id();

    let (response, error_code) = match request.decision {
        PartnershipApplicationDecisionData::Approve => match state
            .approve
            .execute(
                &context,
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await
        {
            Ok(result) => (
                Json(AdminPartnershipApplicationData::from(result.application)).into_response(),
                None,
            ),
            Err(error) => {
                let error = ApiError::from(error);
                let error_code = error.code();
                (error.into_response(), Some(error_code))
            }
        },
        PartnershipApplicationDecisionData::Reject => match state
            .reject
            .execute(
                &context,
                RejectPartnershipApplicationCommand { application_id },
            )
            .await
        {
            Ok(result) => (
                Json(AdminPartnershipApplicationData::from(result.application)).into_response(),
                None,
            ),
            Err(error) => {
                let error = ApiError::from(error);
                let error_code = error.code();
                (error.into_response(), Some(error_code))
            }
        },
    };

    let status = response.status();
    match error_code {
        None => tracing::info!(
            event = "partnership_application.decision",
            action = "decide_partnership_application",
            actor_type = context.principal.kind(),
            actor_id = actor_id.as_deref().unwrap_or(""),
            target_type = "partnership_application",
            target_id = %application_id,
            partnership_application_id = %application_id,
            decision = decision,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            status = %status,
            outcome = "success",
        ),
        Some(error_code) => tracing::warn!(
            event = "partnership_application.decision",
            action = "decide_partnership_application",
            actor_type = context.principal.kind(),
            actor_id = actor_id.as_deref().unwrap_or(""),
            target_type = "partnership_application",
            target_id = %application_id,
            partnership_application_id = %application_id,
            decision = decision,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            status = %status,
            error_code = %error_code,
            outcome = "failure",
        ),
    }
    no_store(response)
}
