use crate::{
    auth::protected_context,
    error::{ApiError, BAD_QUERY_PARAMETER_VALUE},
    product_listing_management::search::{SearchQuery, SearchResponse},
    state::AdminProductListingsState,
};
use axum::{
    Json,
    extract::{RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};

pub async fn search_admin_product_listings(
    State(state): State<AdminProductListingsState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let request = crate::wire::from_query_str::<SearchQuery>(raw.as_deref().unwrap_or_default())
        .map_err(|_| query_error("query", "Invalid directory query."))
        .and_then(|query| query.into_request().map_err(ApiError::from));
    let request = match request {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };
    match state.search.execute(&context, request).await {
        Ok(value) => Json(SearchResponse::from(value)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn query_error(field: &'static str, detail: &str) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail)
}
