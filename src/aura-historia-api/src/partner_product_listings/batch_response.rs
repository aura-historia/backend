use super::types::PartnerProductFailureData;
use crate::error::ApiError;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use listing_source_core::ListingSourceId;
use product_listing_service::use_cases::commands::partner_product_listing_batches::{
    ProductListingBatchInputError, ProductListingBatchResult,
};

pub(super) fn batch_input_error(error: ProductListingBatchInputError) -> ApiError {
    match error {
        ProductListingBatchInputError::AuthenticationRequired => {
            ApiError::unauthorized(crate::error::INVALID_CREDENTIALS)
        }
        ProductListingBatchInputError::TooManyItems => {
            ApiError::bad_request(crate::error::BAD_BODY_VALUE).with_detail(error.to_string())
        }
    }
}

/// Preserves the synchronous REST failure list and first-error status contract.
pub(super) struct BatchResponse {
    listing_source_id: ListingSourceId,
    successes: usize,
    failures: Vec<PartnerProductFailureData>,
    first_error: Option<ApiError>,
}

impl BatchResponse {
    pub(super) fn from_result<T, E: Into<ApiError>>(
        listing_source_id: ListingSourceId,
        source_ids: Vec<String>,
        result: ProductListingBatchResult<T, E>,
    ) -> Self {
        let mut response = Self::new(listing_source_id);
        for item in result.items {
            response.record(source_ids[item.index].clone(), item.result);
        }
        response
    }

    fn new(listing_source_id: ListingSourceId) -> Self {
        Self {
            listing_source_id,
            successes: 0,
            failures: Vec::new(),
            first_error: None,
        }
    }

    fn record<T, E: Into<ApiError>>(&mut self, source_listing_id: String, result: Result<T, E>) {
        match result {
            Ok(_) => self.successes += 1,
            Err(error) => {
                let error = error.into();
                self.failures.push(PartnerProductFailureData::new(
                    self.listing_source_id,
                    source_listing_id,
                    error.code(),
                ));
                self.first_error.get_or_insert(error);
            }
        }
    }
}

impl IntoResponse for BatchResponse {
    fn into_response(self) -> Response {
        if self.successes == 0
            && let Some(error) = self.first_error
        {
            return error.into_response();
        }
        (StatusCode::OK, Json(self.failures)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_core::{
        product_listing_id::ProductListingKey, source_listing_id::SourceListingId,
    };
    use product_listing_service::use_cases::commands::partner_product_listing_batches::ProductListingBatchItem;
    fn report(
        source: ListingSourceId,
        results: Vec<Result<(), ApiError>>,
    ) -> ProductListingBatchResult<(), ApiError> {
        ProductListingBatchResult {
            items: results
                .into_iter()
                .enumerate()
                .map(|(index, result)| ProductListingBatchItem {
                    index,
                    product_key: ProductListingKey::new(
                        source,
                        SourceListingId::try_from("normalized").unwrap(),
                    ),
                    result,
                })
                .collect(),
        }
    }
    #[tokio::test]
    async fn should_return_first_full_error_when_all_items_fail_and_echo_raw_ids_for_partial_results()
     {
        let source = ListingSourceId::new();
        let response = BatchResponse::from_result(
            source,
            vec!["first".into(), "second".into()],
            report(
                source,
                vec![
                    Err(ApiError::not_found(crate::error::PRODUCT_LISTING_NOT_FOUND)),
                    Err(ApiError::forbidden(crate::error::FORBIDDEN)),
                ],
            ),
        )
        .into_response();
        assert_eq!(StatusCode::NOT_FOUND, response.status());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!("PRODUCT_LISTING_NOT_FOUND", body["error"]);
        let response = BatchResponse::from_result(
            source,
            vec!["first".into(), " raw failure ".into(), "last".into()],
            report(
                source,
                vec![
                    Ok(()),
                    Err(ApiError::forbidden(crate::error::FORBIDDEN)),
                    Ok(()),
                ],
            ),
        )
        .into_response();
        assert_eq!(StatusCode::OK, response.status());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            serde_json::json!([{"listingSourceId":source,"sourceListingId":" raw failure ","error":"FORBIDDEN"}]),
            body
        );
    }
}
