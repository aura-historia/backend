use super::types::PartnerProductFailureData;
use crate::error::ApiError;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use listing_source_core::ListingSourceId;

/// Preserves the synchronous REST failure list and first-error status contract.
pub(super) struct BatchResponse {
    listing_source_id: ListingSourceId,
    successes: usize,
    failures: Vec<PartnerProductFailureData>,
    first_error: Option<ApiError>,
}

impl BatchResponse {
    pub(super) fn new(listing_source_id: ListingSourceId) -> Self {
        Self {
            listing_source_id,
            successes: 0,
            failures: Vec::new(),
            first_error: None,
        }
    }

    pub(super) fn record<T, E: Into<ApiError>>(
        &mut self,
        source_listing_id: String,
        result: Result<T, E>,
    ) {
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
