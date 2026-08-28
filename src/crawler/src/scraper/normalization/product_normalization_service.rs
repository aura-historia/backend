pub use super::error::NormalizationError;
use super::{
    datetime::normalize_datetime_field,
    image::normalize_images,
    language::detect_language,
    price::normalize_price_field,
    text::{
        detect_description_language, localize_normalized_title, normalize_description,
        normalize_source_listing_id_with_url_sha_fallback, normalize_title,
    },
};
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::normalization::{
    listing_availability_mapping_service::{
        ListingAvailabilityMappingService, ListingAvailabilityMappingServiceError,
    },
    product::NormalizedProduct,
};

use localization::{Language, Localized};
use money::Currency;
use product_listing_core::{
    description::Description, product_listing_image::ProductListingImage,
    source_listing_id::SourceListingId, title::Title,
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
    /// Availability is resolved from raw extraction text through the injected
    /// [`ListingAvailabilityMappingService`]. Callers do not pre-resolve it; this
    /// method performs the required asynchronous boundary work.
    ///
    /// When `raw.source_listing_id` is blank after trimming, a SHA-256 hash of
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
    /// the availability-mapping LLM fallback was invoked for an unseen raw value,
    /// and `0` otherwise. Callers use this count against the per-shop LLM budget.
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

/// Deterministic candidate-local product data. Availability mapping is intentionally absent.
#[derive(Debug, Clone)]
pub struct PreparedProduct {
    pub source_listing_id: SourceListingId,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<money::Price>,
    pub price_estimate_min: Option<money::Price>,
    pub price_estimate_max: Option<money::Price>,
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
    if state_len > crate::scraper::normalization::listing_availability_mapping_service::MAX_AVAILABILITY_RAW_LEN {
        return Err(NormalizationError::StateTextTooLong {
            len: state_len,
            max: crate::scraper::normalization::listing_availability_mapping_service::MAX_AVAILABILITY_RAW_LEN,
        });
    }
    let source_listing_id =
        normalize_source_listing_id_with_url_sha_fallback(&raw.source_listing_id, &url);
    let title = normalize_title(&raw.title)?;
    let title_language = detect_language(title.as_ref());
    let description_language = detect_description_language(&raw.description);
    let title = localize_normalized_title(title, title_language, description_language)?;
    let description = normalize_description(raw.description, title_language)?;
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
        source_listing_id,
        title,
        description,
        price,
        price_estimate_min,
        price_estimate_max,
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
    listing_availability_mapping_service: Box<dyn ListingAvailabilityMappingService + Send + Sync>,
}

impl ProductListingNormalizationServiceImpl {
    pub fn new(
        listing_availability_mapping_service: Box<
            dyn ListingAvailabilityMappingService + Send + Sync,
        >,
    ) -> Self {
        Self {
            listing_availability_mapping_service,
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
            source_listing_id = %raw.source_listing_id,
            title = %raw.title,
            state = %raw.state,
            price = ?raw.price,
            price_estimate_min = ?raw.price_estimate_min,
            price_estimate_max = ?raw.price_estimate_max,
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

        // Resolve availability only after deterministic candidate validation. This
        // avoids DB/LLM work for candidates that cannot become a listing.
        let (availability_mapping, state_llm_called) = self
            .listing_availability_mapping_service
            .get_listing_availability_mapping(&prepared.raw_state)
            .await
            .map_err(|error| NormalizationFailure {
                llm_calls_used: match &error {
                    ListingAvailabilityMappingServiceError::LargeLanguageModelError(_)
                    | ListingAvailabilityMappingServiceError::UnparsableResponse
                    | ListingAvailabilityMappingServiceError::DatabaseErrorAfterLlm(_) => 1,
                    ListingAvailabilityMappingServiceError::RawStateTooLong { .. }
                    | ListingAvailabilityMappingServiceError::ResponseJsonSchemaSerialization(_)
                    | ListingAvailabilityMappingServiceError::DatabaseError(_) => 0,
                },
                error: match error {
                    ListingAvailabilityMappingServiceError::RawStateTooLong { len, max } => {
                        NormalizationError::StateTextTooLong { len, max }
                    }
                    other => NormalizationError::ListingAvailabilityMappingError(other),
                },
            })?;
        let llm_calls_used = u32::from(state_llm_called);

        Ok(NormalizationSuccess {
            product: NormalizedProduct {
                source_listing_id: prepared.source_listing_id,
                title: prepared.title,
                description: prepared.description,
                price: prepared.price,
                price_estimate_min: prepared.price_estimate_min,
                price_estimate_max: prepared.price_estimate_max,
                availability: availability_mapping,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::normalization::listing_availability_mapping::ListingAvailabilityMapping;
    use crate::scraper::normalization::listing_availability_mapping_service::{
        ListingAvailabilityMappingServiceError, MockListingAvailabilityMappingService,
    };
    use product_listing_core::listing_availability::ListingAvailability;

    fn base_url() -> Url {
        Url::parse("https://example.com/listings/123").unwrap()
    }

    fn raw() -> RawExtractedProduct {
        RawExtractedProduct {
            source_listing_id: "listing-123".into(),
            title: "Antique ceramic vase from the early twentieth century".into(),
            description: vec![],
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: "availability text".into(),
            images: vec![],
            auction_start: None,
            auction_end: None,
            raw_attributes: Default::default(),
        }
    }

    fn service(mapping: ListingAvailabilityMapping) -> ProductListingNormalizationServiceImpl {
        let mut mapping_service = MockListingAvailabilityMappingService::new();
        mapping_service
            .expect_get_listing_availability_mapping()
            .returning(move |_| Box::pin(async move { Ok((mapping, false)) }));
        ProductListingNormalizationServiceImpl::new(Box::new(mapping_service))
    }

    #[tokio::test]
    async fn should_keep_reliable_availability_mapping() {
        let result = service(ListingAvailabilityMapping::Availability(
            ListingAvailability::InStock,
        ))
        .normalize(raw(), base_url(), None)
        .await
        .unwrap();

        assert_eq!(
            result.product.availability,
            ListingAvailabilityMapping::Availability(ListingAvailability::InStock)
        );
        assert_eq!(result.llm_calls_used, 0);
    }

    #[tokio::test]
    async fn should_keep_no_assertion_mapping() {
        let result = service(ListingAvailabilityMapping::NoAssertion)
            .normalize(raw(), base_url(), None)
            .await
            .unwrap();

        assert_eq!(
            result.product.availability,
            ListingAvailabilityMapping::NoAssertion
        );
    }

    #[tokio::test]
    async fn should_keep_ignore_mapping_without_converting_it_to_no_assertion() {
        let result = service(ListingAvailabilityMapping::Ignore)
            .normalize(raw(), base_url(), None)
            .await
            .unwrap();

        assert_eq!(
            result.product.availability,
            ListingAvailabilityMapping::Ignore
        );
    }

    #[tokio::test]
    async fn should_not_map_availability_when_deterministic_validation_fails() {
        let mut mapping_service = MockListingAvailabilityMappingService::new();
        mapping_service
            .expect_get_listing_availability_mapping()
            .times(0);
        let service = ProductListingNormalizationServiceImpl::new(Box::new(mapping_service));
        let mut invalid = raw();
        invalid.title.clear();

        let error = service
            .normalize(invalid, base_url(), None)
            .await
            .unwrap_err();

        assert!(matches!(error.error, NormalizationError::TitleEmpty));
        assert_eq!(error.llm_calls_used, 0);
    }

    #[tokio::test]
    async fn should_count_mapping_llm_call_when_boundary_service_uses_llm() {
        let mut mapping_service = MockListingAvailabilityMappingService::new();
        mapping_service
            .expect_get_listing_availability_mapping()
            .returning(|_| {
                Box::pin(async {
                    Ok((
                        ListingAvailabilityMapping::Availability(ListingAvailability::Available),
                        true,
                    ))
                })
            });
        let service = ProductListingNormalizationServiceImpl::new(Box::new(mapping_service));

        let result = service.normalize(raw(), base_url(), None).await.unwrap();

        assert_eq!(result.llm_calls_used, 1);
    }

    #[tokio::test]
    async fn should_preserve_mapping_service_error() {
        let mut mapping_service = MockListingAvailabilityMappingService::new();
        mapping_service
            .expect_get_listing_availability_mapping()
            .returning(|_| {
                Box::pin(async {
                    Err(ListingAvailabilityMappingServiceError::DatabaseError(
                        sqlx::Error::RowNotFound,
                    ))
                })
            });
        let service = ProductListingNormalizationServiceImpl::new(Box::new(mapping_service));

        let error = service
            .normalize(raw(), base_url(), None)
            .await
            .unwrap_err();

        assert!(matches!(
            error.error,
            NormalizationError::ListingAvailabilityMappingError(_)
        ));
        assert_eq!(error.llm_calls_used, 0);
    }
}
