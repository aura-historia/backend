use super::{types::OwnPartnershipApplicationData, util::no_store};
use crate::{auth::protected_context, error::ApiError, state::PartnershipApplicationsState};
use axum::{
    Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use partnership_service::use_cases::queries::list_own_partnership_applications::ListOwnPartnershipApplicationsRequest;

pub(super) async fn list_own(
    State(state): State<PartnershipApplicationsState>,
    headers: HeaderMap,
) -> Response {
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };

    match state
        .list_own
        .execute(&context, ListOwnPartnershipApplicationsRequest { user_id })
        .await
    {
        Ok(result) => no_store(
            Json(
                result
                    .items
                    .into_iter()
                    .map(OwnPartnershipApplicationData::from)
                    .collect::<Vec<_>>(),
            )
            .into_response(),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}
