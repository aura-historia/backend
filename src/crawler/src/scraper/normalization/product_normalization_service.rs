pub use super::error::NormalizationError;
use super::{
    datetime::normalize_datetime_field,
    image::normalize_images,
    language::detect_language,
    price::normalize_price_field,
    text::{
        detect_description_language, localize_normalized_title, normalize_description,
        normalize_shop_listing_id_with_url_sha_fallback, normalize_title,
    },
};
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::normalization::{
    product::NormalizedProduct,
    state_mapping_service::{ProductStateMappingService, StateMappingServiceError},
};

use localization::{Language, Localized};
use money::Currency;
use product_listing_core::{
    description::Description, product_listing_image::ProductListingImage,
    shop_listing_id::ShopListingId, title::Title,
};

use tracing::debug;
use url::Url;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductListingNormalizationService {
    /// Normalise a raw extracted product.
    ///
    /// The state is resolved automatically from `raw.state` via the injected
    /// [`ProductStateMappingService`]. Callers do not need to pre-resolve the
    /// state; this method handles all async DB/LLM work internally.
    ///
    /// When `raw.shop_listing_id` is blank after trimming, a SHA-256 hash of
    /// the full `url` string is used as a stable fallback identifier rather
    /// than returning an error. This keeps the scrape pipeline alive on pages
    /// where the CSS selector does not extract a product ID.
    ///
    /// `default_currency` is used as a fallback when the raw price string
    /// contains no currency symbol or ISO code (e.g. bare "18,00" on a site
    /// where EUR is implied).  It is set by the LLM during schema
    /// creation/fixing and stored in the [`ProductCssSelectorSchema`].
    ///
    /// Returns `llm_calls_used` with either success or failure.  The value is
    /// `1` when
    /// the state-mapping LLM fallback was invoked (new raw state string not
    /// found in the DB), and `0` otherwise.  Callers use this count to charge
    /// the resolved state against the per-shop LLM budget.
    async fn normalize(
        &self,
        raw: RawExtractedProduct,
        url: Url,
        default_currency: Option<Currency>,
    ) -> ProductListingNormalizationResult;
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizationSuccess {
    pub product: NormalizedProduct,
    pub llm_calls_used: u32,
}

#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct NormalizationFailure {
    pub error: NormalizationError,
    pub llm_calls_used: u32,
}

pub type ProductListingNormalizationResult = Result<NormalizationSuccess, NormalizationFailure>;

/// Deterministic candidate-local product data. State mapping is intentionally absent.
#[derive(Debug, Clone)]
pub struct PreparedProduct {
    pub shop_listing_id: ShopListingId,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<money::Price>,
    pub price_estimate_min: Option<money::Price>,
    pub price_estimate_max: Option<money::Price>,
    pub seller_name: Option<String>,
    pub images: Vec<ProductListingImage>,
    pub auction_start: Option<time::OffsetDateTime>,
    pub auction_end: Option<time::OffsetDateTime>,
    pub raw_attributes: std::collections::BTreeMap<String, Vec<String>>,
    pub raw_state: String,
    pub url: Url,
}

/// Apply deterministic normalization without database or LLM work.
pub fn prepare_product(
    raw: RawExtractedProduct,
    url: Url,
    default_currency: Option<Currency>,
) -> Result<PreparedProduct, NormalizationError> {
    let state_len = raw.state.trim().len();
    if state_len > crate::scraper::normalization::state_mapping_service::MAX_STATE_RAW_LEN {
        return Err(NormalizationError::StateTextTooLong {
            len: state_len,
            max: crate::scraper::normalization::state_mapping_service::MAX_STATE_RAW_LEN,
        });
    }
    let shop_listing_id =
        normalize_shop_listing_id_with_url_sha_fallback(&raw.shop_listing_id, &url);
    let title = normalize_title(&raw.title)?;
    let title_language = detect_language(title.as_ref());
    let description_language = detect_description_language(&raw.description);
    let title = localize_normalized_title(title, title_language, description_language)?;
    let description = normalize_description(raw.description, title_language)?;
    let seller_name = raw.seller_name.and_then(|value| match value.trim() {
        "" => None,
        trimmed => Some(trimmed.to_string()),
    });
    let price = normalize_price_field(
        raw.price,
        "price",
        &url,
        default_currency,
        |r| NormalizationError::PriceUnknownCurrency { raw: r },
        |r| NormalizationError::PriceParseError { raw: r },
    )?;
    let price_estimate_min = normalize_price_field(
        raw.price_estimate_min,
        "price_estimate_min",
        &url,
        default_currency,
        |r| NormalizationError::PriceEstimateMinUnknownCurrency { raw: r },
        |r| NormalizationError::PriceEstimateMinParseError { raw: r },
    )?;
    let price_estimate_max = normalize_price_field(
        raw.price_estimate_max,
        "price_estimate_max",
        &url,
        default_currency,
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
    Ok(PreparedProduct {
        shop_listing_id,
        title,
        description,
        price,
        price_estimate_min,
        price_estimate_max,
        seller_name,
        images,
        auction_start,
        auction_end,
        raw_attributes: raw.raw_attributes,
        raw_state: raw.state,
        url,
    })
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

pub struct ProductListingNormalizationServiceImpl {
    state_mapping_service: Box<dyn ProductStateMappingService + Send + Sync>,
}

impl ProductListingNormalizationServiceImpl {
    pub fn new(state_mapping_service: Box<dyn ProductStateMappingService + Send + Sync>) -> Self {
        Self {
            state_mapping_service,
        }
    }
}

#[async_trait::async_trait]
impl ProductListingNormalizationService for ProductListingNormalizationServiceImpl {
    #[tracing::instrument(skip(self, raw), fields(url = %url))]
    async fn normalize(
        &self,
        raw: RawExtractedProduct,
        url: Url,
        default_currency: Option<Currency>,
    ) -> ProductListingNormalizationResult {
        debug!(
            shop_listing_id = %raw.shop_listing_id,
            title = %raw.title,
            state = %raw.state,
            price = ?raw.price,
            price_estimate_min = ?raw.price_estimate_min,
            price_estimate_max = ?raw.price_estimate_max,
            seller_name = ?raw.seller_name,
            images_count = raw.images.len(),
            has_description = !raw.description.is_empty(),
            has_auction_start = raw.auction_start.is_some(),
            has_auction_end = raw.auction_end.is_some(),
            "Normalizing raw extracted product"
        );
        let prepared =
            prepare_product(raw, url, default_currency).map_err(|error| NormalizationFailure {
                error,
                llm_calls_used: 0,
            })?;

        // Resolve state only after deterministic candidate validation. This
        // avoids DB/LLM work for candidates that cannot become a product.
        let (state_record, state_llm_called) = self
            .state_mapping_service
            .get_state_mapping(&prepared.raw_state)
            .await
            .map_err(|error| NormalizationFailure {
                llm_calls_used: match &error {
                    StateMappingServiceError::LargeLanguageModelError(_)
                    | StateMappingServiceError::UnparsableResponse
                    | StateMappingServiceError::DatabaseErrorAfterLlm(_) => 1,
                    StateMappingServiceError::RawStateTooLong { .. }
                    | StateMappingServiceError::ResponseJsonSchemaSerialization(_)
                    | StateMappingServiceError::DatabaseError(_) => 0,
                },
                error: match error {
                    StateMappingServiceError::RawStateTooLong { len, max } => {
                        NormalizationError::StateTextTooLong { len, max }
                    }
                    other => NormalizationError::StateMappingError(other),
                },
            })?;
        let availability = state_record.normalized;
        let llm_calls_used = u32::from(state_llm_called);

        Ok(NormalizationSuccess {
            product: NormalizedProduct {
                shop_listing_id: prepared.shop_listing_id,
                title: prepared.title,
                description: prepared.description,
                price: prepared.price,
                price_estimate_min: prepared.price_estimate_min,
                price_estimate_max: prepared.price_estimate_max,
                seller_name: prepared.seller_name,
                availability,
                url: prepared.url,
                images: prepared.images,
                auction_start: prepared.auction_start,
                auction_end: prepared.auction_end,
                raw_attributes: prepared.raw_attributes,
            },
            llm_calls_used,
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

    use localization::Language;
    use money::{Currency, MonetaryAmount, Price};
    use product_listing_core::listing_availability::ListingAvailability;
    use time::OffsetDateTime;

    use super::{
        NormalizationError, ProductListingNormalizationService,
        ProductListingNormalizationServiceImpl,
    };
    use crate::scraper::css_selector::product_schema::RawExtractedProduct;
    use crate::scraper::normalization::{
        error::NormalizationFailureScope,
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
            shop_listing_id: "PROD-001".into(),
            // Long enough for lingua to reliably identify as English.
            title: "Antique ceramic vase from the early twentieth century in excellent condition"
                .into(),
            description: vec![],
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            seller_name: None,
            state: "available".into(),
            images: vec![],
            auction_start: None,
            auction_end: None,
            raw_attributes: Default::default(),
        }
    }

    /// Build a mapping record for `raw` resolving to `state_record`.
    fn mapping_record(
        raw: &str,
        state_record: Option<ListingAvailability>,
    ) -> ProductStateMappingRecord {
        let now = OffsetDateTime::now_utc();
        ProductStateMappingRecord {
            raw: raw.to_string(),
            normalized: state_record,
            mapping_type: StateMappingType::Value,
            created: now,
            updated: now,
        }
    }

    /// Create a `ProductListingNormalizationServiceImpl` whose state mapping service
    /// always resolves `raw_state` to `resolved`.
    fn make_service(
        raw_state: &'static str,
        resolved: Option<ListingAvailability>,
    ) -> ProductListingNormalizationServiceImpl {
        let record = mapping_record(raw_state, resolved);
        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping().returning(move |_| {
            let r = record.clone();
            Box::pin(async move { Ok((r, false)) })
        });
        ProductListingNormalizationServiceImpl::new(Box::new(mock))
    }

    /// Create a service whose state mapping service always returns `Available`.
    fn make_available_service() -> ProductListingNormalizationServiceImpl {
        make_service("available", Some(ListingAvailability::Available))
    }

    fn make_service_that_must_not_map_state() -> ProductListingNormalizationServiceImpl {
        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping().times(0);
        ProductListingNormalizationServiceImpl::new(Box::new(mock))
    }

    // -----------------------------------------------------------------------
    // Happy-path tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_normalize_product_when_minimal_raw_provided() {
        let svc = make_available_service();
        let normalized = svc
            .normalize(minimal_raw(), base_url(), None)
            .await
            .unwrap();
        let result = normalized.product;
        let llm_calls = normalized.llm_calls_used;

        assert_eq!(result.shop_listing_id.to_string(), "prod-001");
        assert_eq!(
            result.title.payload.as_ref(),
            "Antique ceramic vase from the early twentieth century in excellent condition"
        );
        assert!(result.description.is_none());
        assert!(result.price.is_none());
        assert!(result.price_estimate_min.is_none());
        assert!(result.price_estimate_max.is_none());
        assert_eq!(result.availability, Some(ListingAvailability::Available));
        assert!(result.images.is_empty());
        assert!(result.auction_start.is_none());
        assert!(result.auction_end.is_none());
        assert_eq!(llm_calls, 0, "DB hit must not count as an LLM call");
    }

    #[tokio::test]
    async fn should_normalize_product_when_full_raw_provided() {
        let svc = make_service("listed", None);
        let raw = RawExtractedProduct {
            shop_listing_id: "LOT-42".into(),
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
            seller_name: Some("Kunstauktionshaus Leipzig | Schütte".into()),
            state: "listed".into(),
            images: vec![
                "https://cdn.example.com/img1.jpg".into(),
                "/img2.jpg".into(),
            ],
            auction_start: Some("2024-06-01T10:00:00Z".into()),
            auction_end: Some("2024-07-01T10:00:00Z".into()),
            raw_attributes: [
                (
                    "rawMaterial".to_string(),
                    vec!["Walnut and brass".to_string()],
                ),
                (
                    "rawMeasurements".to_string(),
                    vec!["H 90 cm x W 45 cm x D 50 cm".to_string()],
                ),
                (
                    "rawOrigin".to_string(),
                    vec!["Southern Germany".to_string()],
                ),
            ]
            .into(),
        };

        let result = svc.normalize(raw, base_url(), None).await.unwrap().product;

        assert_eq!(result.shop_listing_id.to_string(), "lot-42");
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
        assert_eq!(
            result.seller_name.as_deref(),
            Some("Kunstauktionshaus Leipzig | Schütte")
        );
        assert_eq!(result.availability, None);
        assert_eq!(result.images.len(), 2);
        assert_eq!(
            result.auction_start.unwrap(),
            datetime!(2024-06-01 10:00:00 UTC)
        );
        assert_eq!(
            result.auction_end.unwrap(),
            datetime!(2024-07-01 10:00:00 UTC)
        );
        assert_eq!(
            result.raw_attributes.get("rawMaterial"),
            Some(&vec!["Walnut and brass".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawMeasurements"),
            Some(&vec!["H 90 cm x W 45 cm x D 50 cm".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawOrigin"),
            Some(&vec!["Southern Germany".to_string()])
        );
    }

    #[tokio::test]
    async fn should_resolve_state_from_raw_state_field_via_mapping_service() {
        // Each state variant is passed through as-is from the mapping service.
        for (raw_state, state_record, expected) in [
            ("listed", None, None),
            (
                "available",
                Some(ListingAvailability::Available),
                Some(ListingAvailability::Available),
            ),
            (
                "reserved",
                Some(ListingAvailability::Reserved),
                Some(ListingAvailability::Reserved),
            ),
            (
                "sold",
                Some(ListingAvailability::SoldOut),
                Some(ListingAvailability::SoldOut),
            ),
            ("removed", None, None),
            ("unknown", None, None),
        ] {
            let svc = make_service(raw_state, state_record);
            let mut raw = minimal_raw();
            raw.state = raw_state.into();
            let result = svc.normalize(raw, base_url(), None).await.unwrap().product;
            assert_eq!(
                result.availability, expected,
                "state_record {state_record:?} was not converted correctly"
            );
        }
    }

    #[tokio::test]
    async fn should_forward_raw_state_string_to_mapping_service() {
        // Verify that whatever is in raw.state is forwarded verbatim to the
        // mapping service (trimming / lowercasing is the service's concern).
        let raw_state = "  In Stock  ";
        let record = mapping_record(raw_state, Some(ListingAvailability::Available));
        let record_clone = record.clone();

        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping()
            .withf(|s| s == "  In Stock  ")
            .times(1)
            .returning(move |_| {
                let r = record_clone.clone();
                Box::pin(async move { Ok((r, false)) })
            });

        let svc = ProductListingNormalizationServiceImpl::new(Box::new(mock));
        let mut raw = minimal_raw();
        raw.state = raw_state.into();
        let result = svc.normalize(raw, base_url(), None).await.unwrap().product;
        assert_eq!(result.availability, Some(ListingAvailability::Available));
    }

    // -----------------------------------------------------------------------
    // State mapping error propagation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_propagate_state_mapping_error_when_service_fails() {
        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping().times(1).returning(|_| {
            Box::pin(async {
                Err(StateMappingServiceError::DatabaseError(
                    sqlx::Error::RowNotFound,
                ))
            })
        });

        let svc = ProductListingNormalizationServiceImpl::new(Box::new(mock));
        let err = svc
            .normalize(minimal_raw(), base_url(), None)
            .await
            .unwrap_err();
        assert!(
            matches!(err.error, NormalizationError::StateMappingError(_)),
            "expected StateMappingError, got {err:?}"
        );
        assert_eq!(err.llm_calls_used, 0);
        assert_eq!(
            err.error.failure_scope(),
            NormalizationFailureScope::External
        );
    }

    #[tokio::test]
    async fn should_preserve_llm_usage_and_scope_for_unparsable_state_mapping_response() {
        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping()
            .times(1)
            .returning(|_| Box::pin(async { Err(StateMappingServiceError::UnparsableResponse) }));

        let svc = ProductListingNormalizationServiceImpl::new(Box::new(mock));
        let err = svc
            .normalize(minimal_raw(), base_url(), None)
            .await
            .unwrap_err();

        assert!(matches!(
            err.error,
            NormalizationError::StateMappingError(_)
        ));
        assert_eq!(err.llm_calls_used, 1);
        assert_eq!(
            err.error.failure_scope(),
            NormalizationFailureScope::External
        );
    }

    #[tokio::test]
    async fn should_preserve_llm_usage_and_scope_for_database_failure_after_state_mapping_llm() {
        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping().times(1).returning(|_| {
            Box::pin(async {
                Err(StateMappingServiceError::DatabaseErrorAfterLlm(
                    sqlx::Error::RowNotFound,
                ))
            })
        });

        let svc = ProductListingNormalizationServiceImpl::new(Box::new(mock));
        let err = svc
            .normalize(minimal_raw(), base_url(), None)
            .await
            .unwrap_err();

        assert!(matches!(
            err.error,
            NormalizationError::StateMappingError(_)
        ));
        assert_eq!(err.llm_calls_used, 1);
        assert_eq!(
            err.error.failure_scope(),
            NormalizationFailureScope::External
        );
    }

    // -----------------------------------------------------------------------
    // Validation error tests (state resolved to Available for these)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_use_url_sha_as_shop_listing_id_when_extracted_id_is_blank() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.shop_listing_id = "  ".into();
        let url = Url::parse("https://shop.example.com/item/fallback-item").unwrap();
        let result = svc.normalize(raw, url, None).await.unwrap().product;
        assert_eq!(
            result.shop_listing_id.to_string(),
            "3603d78ef2b4963051a2ca8ea12a0b9d774e99baa08a26bdf73916a0261bf198"
        );
    }

    #[tokio::test]
    async fn should_return_error_when_title_is_empty_for_normalize() {
        let svc = make_service_that_must_not_map_state();
        let mut raw = minimal_raw();
        raw.title = "".into();
        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(matches!(err.error, NormalizationError::TitleEmpty));
        assert_eq!(err.llm_calls_used, 0);
    }

    #[tokio::test]
    async fn should_count_one_llm_call_when_state_mapping_uses_llm_after_preparation() {
        let record = mapping_record("available", Some(ListingAvailability::Available));
        let mut mock = MockProductStateMappingService::new();
        mock.expect_get_state_mapping()
            .withf(|raw_state| raw_state == "available")
            .times(1)
            .returning(move |_| {
                let r = record.clone();
                Box::pin(async move { Ok((r, true)) })
            });

        let svc = ProductListingNormalizationServiceImpl::new(Box::new(mock));
        let result = svc
            .normalize(minimal_raw(), base_url(), None)
            .await
            .unwrap();
        assert_eq!(result.llm_calls_used, 1);
        assert_eq!(
            result.product.availability,
            Some(ListingAvailability::Available)
        );
    }

    #[tokio::test]
    async fn should_return_error_when_price_has_no_currency_for_normalize() {
        let svc = make_service_that_must_not_map_state();
        let mut raw = minimal_raw();
        raw.price = Some("1234.56".into());
        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(matches!(
            err.error,
            NormalizationError::PriceUnknownCurrency { .. }
        ));
        assert_eq!(err.llm_calls_used, 0);
    }

    #[tokio::test]
    async fn should_return_error_when_price_is_unparseable_for_normalize() {
        let svc = make_service_that_must_not_map_state();
        let mut raw = minimal_raw();
        raw.price = Some("€".into());
        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(matches!(
            err.error,
            NormalizationError::PriceParseError { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_price_estimate_min_has_no_currency_for_normalize() {
        let svc = make_service_that_must_not_map_state();
        let mut raw = minimal_raw();
        raw.price_estimate_min = Some("800.00".into());
        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(matches!(
            err.error,
            NormalizationError::PriceEstimateMinUnknownCurrency { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_price_estimate_min_is_unparseable_for_normalize() {
        let svc = make_service_that_must_not_map_state();
        let mut raw = minimal_raw();
        raw.price_estimate_min = Some("£".into());
        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(matches!(
            err.error,
            NormalizationError::PriceEstimateMinParseError { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_price_estimate_max_has_no_currency_for_normalize() {
        let svc = make_service_that_must_not_map_state();
        let mut raw = minimal_raw();
        raw.price_estimate_max = Some("1200".into());
        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(matches!(
            err.error,
            NormalizationError::PriceEstimateMaxUnknownCurrency { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_price_estimate_max_is_unparseable_for_normalize() {
        let svc = make_service_that_must_not_map_state();
        let mut raw = minimal_raw();
        raw.price_estimate_max = Some("£".into());
        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(matches!(
            err.error,
            NormalizationError::PriceEstimateMaxParseError { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_auction_start_is_unparseable_for_normalize() {
        let svc = make_service_that_must_not_map_state();
        let mut raw = minimal_raw();
        raw.auction_start = Some("yesterday at noon".into());
        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(matches!(
            err.error,
            NormalizationError::AuctionStartParseError { .. }
        ));
        assert_eq!(err.llm_calls_used, 0);
    }

    #[tokio::test]
    async fn should_return_error_when_auction_end_is_unparseable_for_normalize() {
        let svc = make_service_that_must_not_map_state();
        let mut raw = minimal_raw();
        raw.auction_end = Some("next tuesday".into());
        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(matches!(
            err.error,
            NormalizationError::AuctionEndParseError { .. }
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_image_url_is_invalid_for_normalize() {
        let svc = make_service_that_must_not_map_state();
        let mut raw = minimal_raw();
        raw.images = vec!["//".into()];
        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(matches!(
            err.error,
            NormalizationError::InvalidImageUrl { .. }
        ));
    }

    #[tokio::test]
    async fn should_use_url_from_argument_as_product_url_when_normalizing() {
        let svc = make_available_service();
        let url = Url::parse("https://shop.example.com/item/99").unwrap();
        let result = svc
            .normalize(minimal_raw(), url.clone(), None)
            .await
            .unwrap()
            .product;
        assert_eq!(result.url, url);
    }

    #[tokio::test]
    async fn should_skip_none_price_fields_when_raw_prices_are_absent() {
        let svc = make_available_service();
        let result = svc
            .normalize(minimal_raw(), base_url(), None)
            .await
            .unwrap()
            .product;
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
        let result = svc.normalize(raw, base_url(), None).await.unwrap().product;
        assert!(result.price.is_none());
    }

    #[tokio::test]
    async fn should_default_price_to_none_when_raw_price_is_price_on_request() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.price = Some("Price on Request".into());

        let result = svc.normalize(raw, base_url(), None).await.unwrap().product;

        assert!(
            result.price.is_none(),
            "price should be None for 'Price on Request' markers"
        );
    }

    #[tokio::test]
    async fn should_handle_empty_optional_auction_string_when_raw_auction_is_blank() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.auction_start = Some("  ".into());
        raw.auction_end = Some("  ".into());
        let result = svc.normalize(raw, base_url(), None).await.unwrap().product;
        assert!(result.auction_start.is_none());
        assert!(result.auction_end.is_none());
    }

    #[tokio::test]
    async fn should_normalize_blank_seller_name_to_none() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.seller_name = Some("   ".into());

        let result = svc.normalize(raw, base_url(), None).await.unwrap().product;

        assert_eq!(result.seller_name, None);
    }

    #[tokio::test]
    async fn should_use_description_language_for_short_mixed_title() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.title = "La Saintongeoise".into();
        raw.description = vec![
            "This vintage French lithographic poster comes from a private English catalogue description with clear ownership history.".into(),
        ];

        let result = svc.normalize(raw, base_url(), None).await.unwrap().product;
        assert_eq!(result.title.localization, Language::En);
    }

    #[tokio::test]
    async fn should_use_title_language_for_dimension_only_description() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.description = vec!["23-1/2\"18-1/4\"".into()];

        let result = svc.normalize(raw, base_url(), None).await.unwrap().product;
        let description = result.description.unwrap();

        assert_eq!(description.localization, Language::En);
        assert_eq!(description.payload.as_ref(), "23-1/2\"18-1/4\"");
    }

    // -----------------------------------------------------------------------
    // State text validation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_reject_overlong_state_without_mapping_it() {
        let svc = make_service_that_must_not_map_state();
        let mut raw = minimal_raw();
        raw.state =
            "x".repeat(crate::scraper::normalization::state_mapping_service::MAX_STATE_RAW_LEN + 1);

        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(
            matches!(
                err.error,
                NormalizationError::StateTextTooLong {
                    len,
                    max: crate::scraper::normalization::state_mapping_service::MAX_STATE_RAW_LEN,
                } if len == crate::scraper::normalization::state_mapping_service::MAX_STATE_RAW_LEN + 1
            ),
            "expected StateTextTooLong, got {err:?}"
        );
        assert_eq!(err.llm_calls_used, 0);
        assert_eq!(
            err.error.failure_scope(),
            NormalizationFailureScope::CandidateData
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

        let svc = ProductListingNormalizationServiceImpl::new(Box::new(mock));
        let err = svc
            .normalize(minimal_raw(), base_url(), None)
            .await
            .unwrap_err();
        assert!(
            matches!(err.error, NormalizationError::StateMappingError(_)),
            "expected StateMappingError, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // default_currency fallback
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_use_default_currency_as_fallback_when_price_has_no_currency_symbol() {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.price = Some("1.200,00".into()); // bare price, no currency symbol
        let result = svc
            .normalize(raw, base_url(), Some(Currency::Eur))
            .await
            .unwrap()
            .product;
        assert_eq!(
            result.price.unwrap(),
            Price::new(MonetaryAmount::from(120000u64), Currency::Eur)
        );
    }

    #[tokio::test]
    async fn should_return_unknown_currency_error_when_price_has_no_symbol_and_no_default_currency()
    {
        let svc = make_available_service();
        let mut raw = minimal_raw();
        raw.price = Some("1.200,00".into()); // bare price, no currency symbol
        let err = svc.normalize(raw, base_url(), None).await.unwrap_err();
        assert!(
            matches!(err.error, NormalizationError::PriceUnknownCurrency { .. }),
            "expected PriceUnknownCurrency, got {err:?}"
        );
    }
}
