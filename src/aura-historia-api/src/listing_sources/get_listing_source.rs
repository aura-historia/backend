use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::listing_sources::types::ListingSourceData;
use crate::state::ListingSourcesState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use listing_source_core::ListingSourceId;
use listing_source_service::use_cases::queries::get_listing_source::GetListingSourceRequest;

pub async fn get_listing_source(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
) -> Response {
    let listing_source_id = match ListingSourceId::try_from(raw_listing_source_id.as_str()) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("listingSourceId")
                .with_detail("Path parameter 'listingSourceId' must be a UUID.")
                .into_response();
        }
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };

    match state
        .get
        .execute(&context, GetListingSourceRequest::ById(listing_source_id))
        .await
    {
        Ok(result) => axum::Json(ListingSourceData::from(result)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}
