use super::async_batch;
use crate::{
    auth::{TokenAuthenticator, protected_context},
    error::{
        ApiError, BAD_BODY_VALUE, FORBIDDEN, INVALID_CREDENTIALS, LISTING_SOURCE_NOT_FOUND,
        PRODUCT_LISTING_CONCURRENCY_CONFLICT, PRODUCT_LISTING_INTERNAL_ERROR,
        PRODUCT_LISTING_NOT_FOUND, PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE,
        PRODUCT_LISTING_WITHDRAWN, SHIPPING_REFERENCE_INELIGIBLE,
    },
    wire::parse_path_object_id,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::{
    product_listing_id::ProductListingKey, source_listing_id::SourceListingId,
};
use product_listing_ingestion_sqs::delivery::{PublicDeliveryReferences, decode_command_text};
use product_listing_service::use_cases::{
    ChangeProductListingDeliveryUseCase, GetProductListingDeliveryUseCase,
    IndexedProductListingIngestionIntent, ProductListingDeliveryError,
    ProductListingIngestionIntent, ProductListingIngestionSubmission,
    SubmitPartnerProductListingIngestionUseCase,
};
use serde::Serialize;
use std::sync::Arc;
#[derive(Clone)]
pub struct ProductListingDeliveryState {
    pub change: Arc<dyn ChangeProductListingDeliveryUseCase>,
    pub get: Arc<dyn GetProductListingDeliveryUseCase>,
    pub submit: Arc<dyn SubmitPartnerProductListingIngestionUseCase>,
    pub authenticator: Arc<dyn TokenAuthenticator>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryData {
    product_listing_id: ProductListingId,
    expected_version: u64,
    delivery: Option<serde_json::Value>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MutationData {
    product_listing_id: ProductListingId,
    expected_version: u64,
    outcome: &'static str,
}
fn key(source: &str, listing: &str) -> Result<ProductListingKey, ApiError> {
    Ok(ProductListingKey::new(
        parse_path_object_id::<ListingSourceId>(source, "listing_source_id", "ListingSource")?,
        SourceListingId::try_from(listing).map_err(|_| ApiError::bad_request(BAD_BODY_VALUE))?,
    ))
}
fn error(e: ProductListingDeliveryError) -> ApiError {
    match e {
        ProductListingDeliveryError::AuthenticationRequired => {
            ApiError::unauthorized(INVALID_CREDENTIALS)
        }
        ProductListingDeliveryError::Import(source) => source.into(),
        ProductListingDeliveryError::Forbidden => ApiError::forbidden(FORBIDDEN),
        ProductListingDeliveryError::SourceNotFound => {
            ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
        }
        ProductListingDeliveryError::NotFound => ApiError::not_found(PRODUCT_LISTING_NOT_FOUND),
        ProductListingDeliveryError::StaleRevision => {
            ApiError::conflict(PRODUCT_LISTING_CONCURRENCY_CONFLICT)
        }
        ProductListingDeliveryError::ListingWithdrawn => {
            ApiError::conflict(PRODUCT_LISTING_WITHDRAWN)
        }
        ProductListingDeliveryError::IntegrationMismatch
        | ProductListingDeliveryError::ReferenceIneligible => {
            ApiError::conflict(SHIPPING_REFERENCE_INELIGIBLE)
        }
        ProductListingDeliveryError::InvalidInput { .. } => ApiError::bad_request(BAD_BODY_VALUE),
        ProductListingDeliveryError::Unavailable | ProductListingDeliveryError::Transaction(_) => {
            ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
        }
        ProductListingDeliveryError::OperationFailed { .. } => {
            ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
        }
    }
}
pub async fn get(
    State(s): State<ProductListingDeliveryState>,
    Path((source, listing)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let key = match key(&source, &listing) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s.get.execute(&ctx, key).await {
        Ok(view) => {
            let delivery = view
                .delivery
                .as_ref()
                .map(shipping_json::listing_delivery::encode::<PublicDeliveryReferences>)
                .transpose();
            match delivery {
                Ok(delivery) => Json(DeliveryData {
                    product_listing_id: view.product_listing_id,
                    expected_version: view.version.into_inner(),
                    delivery,
                })
                .into_response(),
                Err(_) => {
                    ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR).into_response()
                }
            }
        }
        Err(e) => error(e).into_response(),
    }
}
async fn apply(
    s: ProductListingDeliveryState,
    source: String,
    listing: String,
    headers: HeaderMap,
    body: String,
    asynchronous: bool,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let parsed = (|| {
        let key = key(&source, &listing)?;
        decode_command_text(&body, key).map_err(|_| ApiError::bad_request(BAD_BODY_VALUE))
    })();
    let command = match parsed {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    if asynchronous {
        let idempotency_key = match async_batch::idempotency_key(&headers) {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
        let source = command.product_key.listing_source_id;
        let source_ids = vec![async_batch::safe_source_id(
            command.product_key.source_listing_id.as_ref(),
        )];
        match s
            .submit
            .execute(
                &ctx,
                ProductListingIngestionSubmission {
                    listing_source_id: source,
                    original_input_count: 1,
                    idempotency_key,
                    items: vec![IndexedProductListingIngestionIntent {
                        index: 0,
                        intent: ProductListingIngestionIntent::ChangeDelivery(command),
                    }],
                },
            )
            .await
        {
            Ok(v) => async_batch::report(v, vec![], &source_ids),
            Err(e) => async_batch::submission_error(e),
        }
    } else {
        match s.change.execute(&ctx, command).await {
            Ok(result) => Json(MutationData {
                product_listing_id: result.product_listing_id,
                expected_version: result.version.into_inner(),
                outcome: if result.outcome.changed() {
                    "CHANGED"
                } else {
                    "UNCHANGED"
                },
            })
            .into_response(),
            Err(e) => error(e).into_response(),
        }
    }
}
pub async fn change(
    State(s): State<ProductListingDeliveryState>,
    Path((source, listing)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    apply(s, source, listing, headers, body, false).await
}
pub async fn submit(
    State(s): State<ProductListingDeliveryState>,
    Path((source, listing)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    apply(s, source, listing, headers, body, true).await
}
