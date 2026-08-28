use crate::auth::protected_context;
use crate::error::{ApiError, BAD_PATH_PARAMETER_VALUE};
use crate::listing_sources::types::ListingSourceData;
use crate::state::ListingSourcesState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use listing_source_core::ListingSourceSlugId;
use listing_source_service::use_cases::queries::get_listing_source::GetListingSourceRequest;

pub async fn get_listing_source_by_slug(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    Path(raw_listing_source_slug_id): Path<String>,
) -> Response {
    let listing_source_slug_id = match ListingSourceSlugId::raw(&raw_listing_source_slug_id) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                .with_path_field("listingSourceSlugId")
                .with_detail("Path parameter 'listingSourceSlugId' is invalid.")
                .into_response();
        }
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };

    match state
        .get
        .execute(
            &context,
            GetListingSourceRequest::BySlug(listing_source_slug_id),
        )
        .await
    {
        Ok(result) => axum::Json(ListingSourceData::from(result)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}
