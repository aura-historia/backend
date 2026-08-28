use super::{
    types::AdminPartnershipApplicationData,
    util::{no_store, parse_id},
};
use crate::{auth::protected_context, error::ApiError, state::PartnershipApplicationsState};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use partnership_service::use_cases::queries::get_partnership_application::GetPartnershipApplicationRequest;

pub(super) async fn get(
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
        .get
        .execute(
            &context,
            GetPartnershipApplicationRequest { application_id },
        )
        .await
    {
        Ok(result) => no_store(Json(AdminPartnershipApplicationData::from(result)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}
