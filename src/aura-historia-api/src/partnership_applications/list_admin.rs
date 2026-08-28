use super::{types::AdminPartnershipApplicationData, util::no_store};
use crate::{auth::protected_context, error::ApiError, state::PartnershipApplicationsState};
use axum::{
    Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use partnership_service::use_cases::queries::list_admin_partnership_applications::ListAdminPartnershipApplicationsRequest;

pub(super) async fn list_admin(
    State(state): State<PartnershipApplicationsState>,
    headers: HeaderMap,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };

    match state
        .list_admin
        .execute(&context, ListAdminPartnershipApplicationsRequest)
        .await
    {
        Ok(result) => no_store(
            Json(
                result
                    .items
                    .into_iter()
                    .map(AdminPartnershipApplicationData::from)
                    .collect::<Vec<_>>(),
            )
            .into_response(),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}
