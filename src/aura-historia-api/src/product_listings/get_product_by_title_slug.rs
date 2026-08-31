use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_PATH_PARAMETER_VALUE, BAD_QUERY_PARAMETER_VALUE};
use crate::product_listings::product_data::product_response;
use crate::state::ProductListingsState;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use localization::Language;
use money::Currency;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_service::use_cases::{GetProductListingRequest, ProductListingLookup};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ProductListingDetailsQuery {
    #[serde(default)]
    #[serde(with = "crate::wire::language")]
    language: Language,
    #[serde(default, with = "crate::wire::currency")]
    currency: Currency,
}

pub async fn get_product_by_title_slug(
    State(state): State<ProductListingsState>,
    headers: HeaderMap,
    Path(raw_product_listing_title_slug_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query: ProductListingDetailsQuery =
        match serde_qs::from_str(raw_query.as_deref().unwrap_or_default()) {
            Ok(query) => query,
            Err(error) => {
                return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response();
            }
        };
    let product_listing_title_slug_id =
        match ProductListingSlugId::raw(&raw_product_listing_title_slug_id) {
            Ok(value) => value,
            Err(_) => {
                return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                    .with_path_field("productListingTitleSlugId")
                    .with_detail("Path parameter 'productListingTitleSlugId' is invalid.")
                    .into_response();
            }
        };
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };

    let context = principal.operation_context(metadata);
    match state
        .get_product
        .execute(
            &context,
            GetProductListingRequest {
                lookup: ProductListingLookup::ByTitleSlug(product_listing_title_slug_id),
                language: query.language,
                currency: query.currency,
            },
        )
        .await
    {
        Ok(view) => product_response(view, &context.principal),
        Err(error) => ApiError::from(error).into_response(),
    }
}
