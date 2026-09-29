use super::{
    async_batch,
    types::{UpsertProductListingData, parse_listing_source_id},
};
use crate::state::AsyncPartnerProductListingsState;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use product_listing_service::use_cases::{
    ProductListingIngestionIntent, ProductListingIngestionSubmission,
};

pub async fn upsert_products(
    State(state): State<AsyncPartnerProductListingsState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
    body: String,
) -> Response {
    let listing_source_id = match parse_listing_source_id(&raw_listing_source_id) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let key = match async_batch::idempotency_key(&headers) {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let context =
        match async_batch::authorized_context(state.authenticator.as_ref(), &headers).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    let raw_items = match async_batch::parse_envelope(&body) {
        Ok(items) => items,
        Err(error) => return error.into_response(),
    };
    let count = raw_items.len();
    let (items, source_ids, failures) = async_batch::parse_items::<UpsertProductListingData>(
        raw_items,
        |product| &product.source_listing_id,
        |product| {
            product
                .into_command(listing_source_id)
                .map(ProductListingIngestionIntent::Upsert)
        },
    );
    match state
        .submit
        .execute(
            &context,
            ProductListingIngestionSubmission {
                listing_source_id,
                original_input_count: count,
                idempotency_key: key,
                items,
            },
        )
        .await
    {
        Ok(result) => async_batch::report(result, failures, &source_ids),
        Err(error) => async_batch::submission_error(error),
    }
}

#[cfg(test)]
#[path = "async_upsert_products_tests.rs"]
mod tests;
