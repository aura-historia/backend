pub use super::error::NormalizationError;
use super::{
    datetime::normalize_datetime_field,
    image::normalize_images,
    price::normalize_price_field,
    text::{normalize_description, normalize_shops_product_id, normalize_title_localized},
};
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::normalization::{
    product::NormalizedProduct,
    state_mapping_service::{ProductStateMappingService, StateMappingServiceError},
};

use common::product_state::domain::ProductState;

use url::Url;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductNormalizationService {
    /// Normalise a raw extracted product.
    ///
    /// The state is resolved automatically from `raw.state` via the injected
    /// [`ProductStateMappingService`]. Callers do not need to pre-resolve the
    /// state; this method handles all async DB/LLM work internally.
    async fn normalize(
        &self,
        raw: RawExtractedProduct,
        url: Url,
    ) -> Result<NormalizedProduct, NormalizationError>;
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

pub struct ProductNormalizationServiceImpl {
    state_mapping_service: Box<dyn ProductStateMappingService + Send + Sync>,
}

impl ProductNormalizationServiceImpl {
    pub fn new(state_mapping_service: Box<dyn ProductStateMappingService + Send + Sync>) -> Self {
        Self {
            state_mapping_service,
        }
    }
}

#[async_trait::async_trait]
impl ProductNormalizationService for ProductNormalizationServiceImpl {
    async fn normalize(
        &self,
        raw: RawExtractedProduct,
        url: Url,
    ) -> Result<NormalizedProduct, NormalizationError> {
        // Resolve state first — this is the only async step.
        let state_record = self
            .state_mapping_service
            .get_state_mapping(&raw.state)
            .await
            .map_err(|e| match e {
                StateMappingServiceError::RawStateTooLong { len, max } => {
                    NormalizationError::StateTextTooLong { len, max }
                }
                other => NormalizationError::StateMappingError(other),
            })?
            .normalized;
        let state = ProductState::from(state_record);

        let shops_product_id = normalize_shops_product_id(&raw.shops_product_id)?;
        let title = normalize_title_localized(&raw.title)?;
        let description = normalize_description(raw.description)?;

        let price = normalize_price_field(
            raw.price,
            |r| NormalizationError::PriceUnknownCurrency { raw: r },
            |r| NormalizationError::PriceParseError { raw: r },
        )?;
        let price_estimate_min = normalize_price_field(
            raw.price_estimate_min,
            |r| NormalizationError::PriceEstimateMinUnknownCurrency { raw: r },
            |r| NormalizationError::PriceEstimateMinParseError { raw: r },
        )?;
        let price_estimate_max = normalize_price_field(
            raw.price_estimate_max,
            |r| NormalizationError::PriceEstimateMaxUnknownCurrency { raw: r },
            |r| NormalizationError::PriceEstimateMaxParseError { raw: r },
        )?;

        let images = normalize_images(raw.images, &url)?;

        let auction_start = normalize_datetime_field(raw.auction_start, |r| {
            NormalizationError::AuctionStartParseError { raw: r }
        })?;
        let auction_end = normalize_datetime_field(raw.auction_end, |r| {
            NormalizationError::AuctionEndParseError { raw: r }
        })?;

        Ok(NormalizedProduct {
            shops_product_id,
            title,
            description,
            price,
            price_estimate_min,
            price_estimate_max,
            state,
            url,
            images,
            auction_start,
            auction_end,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use time::macros::datetime;
    use url::Url;

    use common::{
        currency::domain::Currency,
        price::domain::{MonetaryAmount, Price},
        product_state::domain::ProductState,
    };
    use product::dynamodb::product_state_record::ProductStateRecord;
    use time::OffsetDateTime;

    use super::{NormalizationError, ProductNormalizationService, ProductNormalizationServiceImpl};
    use crate::scraper::css_selector::product_schema::RawExtractedProduct;
    use crate::scraper::normalization::{
        state::{ProductStateMappingRecord, StateMappingType},
        state_mapping_service::{MockProductStateMappingService, StateMappingServiceError},
    };
    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn base_url() -> Url {
        Url::parse("https://example.com/products/123").unwrap()
    }

    fn minimal_raw() -> RawExtractedProduct {
        RawExtractedProduct {
            shops_product_id: "PROD-001".into(),
            // Long enough for lingua to reliably identify as English.
            title: "Antique ceramic vase from the early twentieth century in excellent condition"
                .into(),
            description: vec![],
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: "available".into(),
            images: vec![],
            auction_start: None,
            auction_end: None,
        }
    }

    /// Build a mapping record for `raw` resolving to `state_record`.
    fn mapping_record(raw: &str, state_record: ProductStateRecord) -> ProductStateMappingRecord {
        let now = OffsetDateTime::now_utc();
        ProductStateMappingRecord {
            raw: raw.to_string(),
            normalized: state_record,
            mapping_type: StateMappingType::Value,
            created: now,
            updated: now,
        }
    }

    /// Create a `ProductNormalizationServiceImpl` whose state mapping service
    /// always resolves `raw_state` to `resolved`.
    fn make_service(
        raw_state: &'static str,
        resolved: ProductStateRecord,
    ) -> ProductNormalizationServiceImpl {
        let record = mapping_record(raw_state, resolved);
        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping().returning(move |_| {
            let r = record.clone();
            Box::pin(async move { Ok(r) })
        });
        ProductNormalizationServiceImpl::new(Box::new(mock))
    }

    /// Create a service whose state mapping service always returns `Available`.
    fn make_available_service() -> ProductNormalizationServiceImpl {
        make_service("available", ProductStateRecord::Available)
    }

    // -----------------------------------------------------------------------
    // Happy-path tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_normalize_product_when_minimal_raw_provided() {
        let svc = make_available_service();
        let result = svc.normalize(minimal_raw(), base_url()).await.unwrap();

        assert_eq!(result.shops_product_id.to_string(), "PROD-001");
        assert_eq!(
            result.title.payload.as_ref(),
            "Antique ceramic vase from the early twentieth century in excellent condition"
        );
        assert!(result.description.is_none());
        assert!(result.price.is_none());
        assert!(result.price_estimate_min.is_none());
        assert!(result.price_estimate_max.is_none());
        assert_eq!(result.state, ProductState::Available);
        assert!(result.images.is_empty());
        assert!(result.auction_start.is_none());
        assert!(result.auction_end.is_none());
    }

    #[tokio::test]
    async fn should_normalize_product_when_full_raw_provided() {
        let svc = make_service("listed", ProductStateRecord::Listed);
        let raw = RawExtractedProduct {
            shops_product_id: "LOT-42".into(),
            // Long enough English text for reliable language detection.
            title:
                "Victorian silver brooch in excellent original condition from private collection"
                    .into(),
            description: vec![
                "A beautiful antique brooch from the Victorian era.".into(),
                "In excellent original condition with no damage.".into(),
            ],
            price: Some("€ 1.200,00".into()),
            price_estimate_min: Some("£ 800.00".into()),
            price_estimate_max: Some("£1,200.00".into()),
            state: "listed".into(),
            images: vec![
                "https://cdn.example.com/img1.jpg".into(),
                "/img2.jpg".into(),
            ],
            auction_start: Some("2024-06-01T10:00:00Z".into()),
            auction_end: Some("2024-07-01T10:00:00Z".into()),
        };

        let result = svc.normalize(raw, base_url()).await.unwrap();

        assert_eq!(result.shops_product_id.to_string(), "LOT-42");
        assert_eq!(
            result.title.payload.as_ref(),
            "Victorian silver brooch in excellent original condition from private collection"
        );
        assert_eq!(
            result.description.unwrap().payload.as_ref(),
            "A beautiful antique brooch from the Victorian era.\n\nIn excellent original condition with no damage."
        );
        assert_eq!(
            result.price.unwrap(),
            Price::new(MonetaryAmount::from(120000u64), Currency::Eur)
        );
        assert_eq!(
            result.price_estimate_min.unwrap(),
            Price::new(MonetaryAmount::from(80000u64), Currency::Gbp)
        );
        assert_eq!(
            result.price_estimate_max.unwrap(),
            Price::new(MonetaryAmount::from(120000u64), Currency::Gbp)
        );
        assert_eq!(result.state, ProductState::Listed);
        assert_eq!(result.images.len(), 2);
        assert_eq!(
            result.auction_start.unwrap(),
            datetime!(2024-06-01 10:00:00 UTC)
        );
        assert_eq!(
            result.auction_end.unwrap(),
            datetime!(2024-07-01 10:00:00 UTC)
        );
    }

    #[tokio::test]
    async fn should_resolve_state_from_raw_state_field_via_mapping_service() {
        // Each state variant is passed through as-is from the mapping service.
        for (raw_state, state_record, expected) in [
            ("listed", ProductStateRecord::Listed, ProductState::Listed),
            (
                "available",
                ProductStateRecord::Available,
                ProductState::Available,
            ),
            (
                "reserved",
                ProductStateRecord::Reserved,
                ProductState::Reserved,
            ),
            ("sold", ProductStateRecord::Sold, ProductState::Sold),
            (
                "removed",
                ProductStateRecord::Removed,
                ProductState::Removed,
            ),
            (
                "unknown",
                ProductStateRecord::Unknown,
                ProductState::Unknown,
            ),
        ] {
            let svc = make_service(raw_state, state_record);
            let mut raw = minimal_raw();
            raw.state = raw_state.into();
            let result = svc.normalize(raw, base_url()).await.unwrap();
            assert_eq!(
                result.state, expected,
                "state_record {state_record:?} was not converted correctly"
            );
        }
    }

    #[tokio::test]
    async fn should_forward_raw_state_string_to_mapping_service() {
        // Verify that whatever is in raw.state is forwarded verbatim to the
        // mapping service (trimming / lowercasing is the service's concern).
        let raw_state = "  In Stock  ";
        let record = mapping_record(raw_state, ProductStateRecord::Available);
        let record_clone = record.clone();

        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping()
            .withf(|s| s == "  In Stock  ")
            .times(1)
            .returning(move |_| {
                let r = record_clone.clone();
                Box::pin(async move { Ok(r) })
            });

        let svc = ProductNormalizationServiceImpl::new(Box::new(mock));
        let mut raw = minimal_raw();
        raw.state = raw_state.into();
        let result = svc.normalize(raw, base_url()).await.unwrap();
        assert_eq!(result.state, ProductState::Available);
    }

    // -----------------------------------------------------------------------
    // State mapping error propagation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_propagate_state_mapping_error_when_service_fails() {
        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping().returning(|_| {
            Box::pin(async {
                Err(StateMappingServiceError::DatabaseError(
                    sqlx::Error::RowNotFound,
                ))
            })
        });

        let svc = ProductNormalizationServiceImpl::new(Box::new(mock));
        let err = svc.normalize(minimal_raw(), base_url()).await.unwrap_err();
        assert!(
            matches!(err, NormalizationError::StateMappingError(_)),
            "expected StateMappingError, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Validation error tests (state resolved to Available for these)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_error_when_shops_product_id_is_empty_for_normalize() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.shops_product_id = "  ".into();
        let err = svc.normalize(raw, base_url()).await.unwrap_err();
        assert!(matches!(err, NormalizationError::ShopsProductIdEmpty));
    }

    #[tokio::test]
    async fn should_return_error_when_title_is_empty_for_normalize() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.title = "".into();
        let err = svc.normalize(raw, base_url()).await.unwrap_err();
        assert!(matches!(err, NormalizationError::TitleEmpty));
    }

    #[tokio::test]
    async fn should_return_error_when_price_has_no_currency_for_normalize() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.price = Some("1234.56".into());
        let err = svc.normalize(raw, base_url()).await.unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceUnknownCurrency { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_price_is_unparseable_for_normalize() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.price = Some("€".into());
        let err = svc.normalize(raw, base_url()).await.unwrap_err();
        assert!(matches!(err, NormalizationError::PriceParseError { .. }));
    }

    #[tokio::test]
    async fn should_return_error_when_price_estimate_min_has_no_currency_for_normalize() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.price_estimate_min = Some("800.00".into());
        let err = svc.normalize(raw, base_url()).await.unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMinUnknownCurrency { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_price_estimate_min_is_unparseable_for_normalize() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.price_estimate_min = Some("£".into());
        let err = svc.normalize(raw, base_url()).await.unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMinParseError { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_price_estimate_max_has_no_currency_for_normalize() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.price_estimate_max = Some("1200".into());
        let err = svc.normalize(raw, base_url()).await.unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMaxUnknownCurrency { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_price_estimate_max_is_unparseable_for_normalize() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.price_estimate_max = Some("£".into());
        let err = svc.normalize(raw, base_url()).await.unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMaxParseError { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_auction_start_is_unparseable_for_normalize() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.auction_start = Some("yesterday at noon".into());
        let err = svc.normalize(raw, base_url()).await.unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::AuctionStartParseError { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_auction_end_is_unparseable_for_normalize() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.auction_end = Some("next tuesday".into());
        let err = svc.normalize(raw, base_url()).await.unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::AuctionEndParseError { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_image_url_is_invalid_for_normalize() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.images = vec!["//".into()];
        let err = svc.normalize(raw, base_url()).await.unwrap_err();
        assert!(matches!(err, NormalizationError::InvalidImageUrl { .. }));
    }

    #[tokio::test]
    async fn should_use_url_from_argument_as_product_url_when_normalizing() {
        let svc = make_available_service();
        let url = Url::parse("https://shop.example.com/item/99").unwrap();
        let result = svc.normalize(minimal_raw(), url.clone()).await.unwrap();
        assert_eq!(result.url, url);
    }

    #[tokio::test]
    async fn should_skip_none_price_fields_when_raw_prices_are_absent() {
        let svc = make_available_service();
        let result = svc.normalize(minimal_raw(), base_url()).await.unwrap();
        assert!(result.price.is_none());
        assert!(result.price_estimate_min.is_none());
        assert!(result.price_estimate_max.is_none());
    }

    #[tokio::test]
    async fn should_handle_empty_optional_price_string_when_raw_price_is_blank() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.price = Some("  ".into());
        // Blank string treated as absent — no currency error expected.
        let result = svc.normalize(raw, base_url()).await.unwrap();
        assert!(result.price.is_none());
    }

    #[tokio::test]
    async fn should_handle_empty_optional_auction_string_when_raw_auction_is_blank() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.auction_start = Some("  ".into());
        raw.auction_end = Some("  ".into());
        let result = svc.normalize(raw, base_url()).await.unwrap();
        assert!(result.auction_start.is_none());
        assert!(result.auction_end.is_none());
    }

    // -----------------------------------------------------------------------
    // RawStateTooLong → StateTextTooLong conversion
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_map_raw_state_too_long_to_state_text_too_long_normalization_error() {
        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping().returning(|_| {
            Box::pin(async {
                Err(StateMappingServiceError::RawStateTooLong {
                    len: 1024,
                    max: 512,
                })
            })
        });

        let svc = ProductNormalizationServiceImpl::new(Box::new(mock));
        let err = svc.normalize(minimal_raw(), base_url()).await.unwrap_err();
        assert!(
            matches!(
                err,
                NormalizationError::StateTextTooLong {
                    len: 1024,
                    max: 512
                }
            ),
            "expected StateTextTooLong, got {err:?}"
        );
    }

    #[tokio::test]
    async fn should_map_other_state_mapping_errors_to_state_mapping_error() {
        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping().returning(|_| {
            Box::pin(async {
                Err(StateMappingServiceError::DatabaseError(
                    sqlx::Error::RowNotFound,
                ))
            })
        });

        let svc = ProductNormalizationServiceImpl::new(Box::new(mock));
        let err = svc.normalize(minimal_raw(), base_url()).await.unwrap_err();
        assert!(
            matches!(err, NormalizationError::StateMappingError(_)),
            "expected StateMappingError, got {err:?}"
        );
    }
}
