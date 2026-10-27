use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE, LISTING_SOURCE_INTERNAL_ERROR};
use crate::listing_sources::types::{CreateListingSourceData, ListingSourceReferenceData};
use crate::state::ListingSourcesState;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use listing_source_service::use_cases::commands::create_listing_source::CreateListingSourceCommand;

pub async fn create_listing_source(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let data = match parse_body(&body) {
        Ok(data) => data,
        Err(error) => return error.into_response(),
    };
    let command = match CreateListingSourceCommand::try_from(data) {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };

    match state.create.execute(&context, command).await {
        Ok(result) => {
            let location = format!("/v1/admin/listing-sources/{}", result.listing_source_id);
            let location = match HeaderValue::from_str(&location) {
                Ok(value) => value,
                Err(_) => {
                    return ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                        .with_detail("Listing source location failed internally.")
                        .into_response();
                }
            };
            let mut response = (
                StatusCode::CREATED,
                axum::Json(ListingSourceReferenceData::from((
                    result.listing_source_id,
                    result.slug_id,
                ))),
            )
                .into_response();
            response.headers_mut().insert(header::LOCATION, location);
            response
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_body(body: &str) -> Result<CreateListingSourceData, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}
