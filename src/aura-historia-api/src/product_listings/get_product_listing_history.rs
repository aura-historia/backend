use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, PRODUCT_LISTING_INTERNAL_ERROR};
use crate::product_listings::product_listing_history_entry_data::ProductListingHistoryEntryData;
use crate::state::ProductListingsState;
use crate::wire::parse_path_object_id;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use money::Currency;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::use_cases::{
    GetProductListingHistoryRequest, ProductListingHistoryLookup,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ProductListingHistoryQuery {
    #[serde(default, with = "crate::wire::currency::option")]
    currency: Option<Currency>,
}

pub async fn get_product_listing_history_by_id(
    State(state): State<ProductListingsState>,
    headers: HeaderMap,
    Path(raw_product_listing_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query: ProductListingHistoryQuery =
        match serde_qs::from_str(raw_query.as_deref().unwrap_or_default()) {
            Ok(query) => query,
            Err(error) => {
                return crate::transport::cache::private_no_store(
                    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                        .with_detail(error.to_string())
                        .into_response(),
                );
            }
        };
    let product_listing_id = match parse_path_object_id::<ProductListingId>(
        &raw_product_listing_id,
        "productListingId",
        "ProductListing",
    ) {
        Ok(id) => id,
        Err(error) => return crate::transport::cache::private_no_store(error.into_response()),
    };
    history_response(
        state,
        headers,
        ProductListingHistoryLookup::ById(product_listing_id),
        query.currency,
    )
    .await
}

async fn history_response(
    state: ProductListingsState,
    headers: HeaderMap,
    lookup: ProductListingHistoryLookup,
    currency: Option<Currency>,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            return crate::transport::cache::private_no_store(
                ApiError::from(error).into_response(),
            );
        }
    };
    let Some(use_case) = state.get_product_listing_history.as_ref() else {
        return crate::transport::cache::private_no_store(
            ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                .with_detail("ProductListing history is not configured.")
                .into_response(),
        );
    };
    let context = principal.operation_context(metadata);
    match use_case
        .execute(
            &context,
            GetProductListingHistoryRequest { lookup, currency },
        )
        .await
    {
        Ok(history) => crate::transport::cache::anonymous_shared_success(
            Json(
                history
                    .into_iter()
                    .map(ProductListingHistoryEntryData::from)
                    .collect::<Vec<_>>(),
            )
            .into_response(),
            &headers,
            matches!(&principal, crate::auth::TransportPrincipal::Anonymous),
            300,
        ),
        Err(error) => {
            crate::transport::cache::private_no_store(ApiError::from(error).into_response())
        }
    }
}
