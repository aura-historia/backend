//! Public advertised terms and private assessment fields on existing listing responses.
use crate::error::{ApiError, FORBIDDEN, SHIPPING_STORAGE_ERROR, SHIPPING_TEMPORARILY_UNAVAILABLE};
use product_listing_service::shipping::AdvertisedShipping;
#[cfg(test)]
use serde_json::Value;
use shipping_service::assessment::AssessmentError;
use time::OffsetDateTime;

impl From<product_listing_service::shipping::ShippingReadError> for ApiError {
    fn from(error: product_listing_service::shipping::ShippingReadError) -> Self {
        use product_listing_service::shipping::ShippingReadError;
        match error {
            ShippingReadError::Assessment(error) => error.into(),
            ShippingReadError::FxSnapshotMissing
            | ShippingReadError::FxRead(
                fxrate_service::ports::FxRateSnapshotReadError::ReadFailed { .. },
            ) => Self::service_unavailable(SHIPPING_TEMPORARILY_UNAVAILABLE),
            ShippingReadError::Conversion(_) | ShippingReadError::FxRead(_) => {
                Self::internal_server_error(SHIPPING_STORAGE_ERROR)
            }
        }
    }
}

impl From<AssessmentError> for ApiError {
    fn from(error: AssessmentError) -> Self {
        match error {
            AssessmentError::InvalidInput | AssessmentError::InvalidState(_) => {
                Self::internal_server_error(SHIPPING_STORAGE_ERROR)
            }
            AssessmentError::Forbidden => Self::forbidden(FORBIDDEN),
            AssessmentError::Destination(error) => error.into(),
            AssessmentError::Storage(_) | AssessmentError::Transaction(_) => {
                Self::service_unavailable(SHIPPING_TEMPORARILY_UNAVAILABLE)
            }
        }
    }
}

/// Cap shared caching at geographic provider evidence retention deadlines.
pub(crate) fn cache_ttl<'a>(
    items: impl IntoIterator<Item = Option<&'a AdvertisedShipping>>,
    maximum: u32,
) -> u32 {
    let now = OffsetDateTime::now_utc();
    items.into_iter().flatten().fold(maximum, |ttl, item| {
        item.geographic_valid_until
            .into_iter()
            .filter(|deadline| *deadline > item.evaluated_at)
            .fold(ttl, |ttl, deadline| {
                ttl.min(u32::try_from((deadline - now).whole_seconds().max(0)).unwrap_or(u32::MAX))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::static_error;
    use axum::{body::to_bytes, http::StatusCode, response::IntoResponse};
    use fxrate_core::FxRateSnapshotError;
    use fxrate_service::ports::FxRateSnapshotReadError;
    use product_listing_service::shipping::ShippingReadError;

    #[tokio::test]
    async fn shipping_fx_failures_keep_their_documented_status_and_bounded_public_error() {
        for (error, status, code) in [
            (
                ShippingReadError::FxSnapshotMissing,
                StatusCode::SERVICE_UNAVAILABLE,
                SHIPPING_TEMPORARILY_UNAVAILABLE,
            ),
            (
                ShippingReadError::FxRead(FxRateSnapshotReadError::ReadFailed {
                    source: static_error("private storage diagnostic"),
                }),
                StatusCode::SERVICE_UNAVAILABLE,
                SHIPPING_TEMPORARILY_UNAVAILABLE,
            ),
            (
                ShippingReadError::FxRead(FxRateSnapshotReadError::InvalidPersistedSnapshot {
                    source: static_error("private snapshot diagnostic"),
                }),
                StatusCode::INTERNAL_SERVER_ERROR,
                SHIPPING_STORAGE_ERROR,
            ),
            (
                ShippingReadError::Conversion(FxRateSnapshotError::MissingQuote(
                    money::Currency::Jpy,
                )),
                StatusCode::INTERNAL_SERVER_ERROR,
                SHIPPING_STORAGE_ERROR,
            ),
            (
                ShippingReadError::Conversion(FxRateSnapshotError::ConversionOverflow),
                StatusCode::INTERNAL_SERVER_ERROR,
                SHIPPING_STORAGE_ERROR,
            ),
        ] {
            let error = ApiError::from(error);
            assert_eq!(code, error.code());
            let response = error.into_response();
            assert_eq!(status, response.status());
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(status.as_u16(), body["status"]);
            assert!(body.get("source").is_none());
            assert!(body.get("detail").is_none());
        }
    }
}
