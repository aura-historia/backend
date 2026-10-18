pub use super::error::NormalizationError;

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use money::Currency;
use product_listing_normalization::diagnostics::{
    availability_normalization_error, price_normalization_error,
};
use product_listing_normalization::{
    ImageUrlNormalizationError, ListingAvailabilityQuickCheck, NormalizationField,
    NormalizationIssue, NormalizationIssueDetails, NormalizationIssueReason, NormalizationReport,
    PriceField, PriceNormalizationError, normalize_description, normalize_image_urls,
    normalize_price, normalize_product_listing_price,
    normalize_source_listing_id_with_url_sha_fallback, normalize_title, quick_check_availability,
};
use tracing::debug;
use url::Url;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductListingNormalizationService: Send + Sync {
    /// Normalizes crawler-extracted values using only the shared pure kernel.
    async fn normalize(
        &self,
        raw: RawExtractedProduct,
        context: ProductNormalizationContext,
    ) -> ProductListingNormalizationResult;
}

/// Stable and effective identity context for one crawler normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductNormalizationContext {
    pub effective_url: Url,
    pub source_record_key: String,
    pub fallback_currency: Option<Currency>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizationSuccess {
    /// Candidate-local prepared values. They support plausibility and ranking;
    /// canonical writes use the separate raw-input handoff.
    pub prepared: PreparedProduct,
    /// Deterministic preparation never consumes LLM budget.
    pub llm_calls_used: u32,
}

#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct NormalizationFailure {
    pub error: NormalizationError,
    pub llm_calls_used: u32,
}

pub type ProductListingNormalizationResult = Result<NormalizationSuccess, NormalizationFailure>;

/// Deterministic candidate-local product data. Source mapping stays in crawler.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedProduct {
    /// Pure crawler outcome used only for local disposition.
    pub availability: product_listing_normalization::ListingAvailabilityQuickCheck,
    pub source_listing_id: product_listing_core::source_listing_id::SourceListingId,
    pub title: localization::Localized<localization::Language, product_listing_core::title::Title>,
    pub description: Option<
        localization::Localized<
            localization::Language,
            product_listing_core::description::Description,
        >,
    >,
    pub price: Option<product_listing_core::product_listing_price::ProductListingPrice>,
    pub price_estimate_min: Option<money::Price>,
    pub price_estimate_max: Option<money::Price>,
    pub images: Vec<product_listing_core::product_listing_image::ProductListingImage>,
    pub raw_attributes: std::collections::BTreeMap<String, Vec<String>>,
    pub raw_state: String,
    pub url: Url,
}

/// Candidate-local preparation decision plus every observed issue. Replay calls this function.
#[derive(Debug)]
pub struct CrawlerPreparationEvaluation {
    pub outcome: Result<PreparedProduct, NormalizationError>,
    pub report: NormalizationReport,
}

pub fn prepare_product(
    raw: RawExtractedProduct,
    context: ProductNormalizationContext,
) -> Result<PreparedProduct, NormalizationError> {
    evaluate_preparation(raw, context).outcome
}

/// Evaluates every independent field. The decision keeps the first failure in field order;
/// the report retains every hard failure and soft miss.
pub fn evaluate_preparation(
    raw: RawExtractedProduct,
    context: ProductNormalizationContext,
) -> CrawlerPreparationEvaluation {
    let ProductNormalizationContext {
        effective_url: url,
        source_record_key,
        fallback_currency,
    } = context;
    let mut report = NormalizationReport::new();
    let mut first_error = None;
    let mut fail = |(error, issue): (NormalizationError, NormalizationIssue),
                    report: &mut NormalizationReport| {
        report.push(issue);
        first_error.get_or_insert(error);
    };

    let availability = match quick_check_availability(raw.state.as_str()) {
        Ok(availability) => {
            if let ListingAvailabilityQuickCheck::Unsupported(reason) = availability {
                report.push(NormalizationIssue::availability_unsupported(reason));
            }
            Some(availability)
        }
        Err(error) => {
            let error = availability_normalization_error(error);
            fail(reject(error), &mut report);
            None
        }
    };
    let source_listing_id = match Url::parse(&source_record_key) {
        Ok(raw_source_record_url) => match normalize_source_listing_id_with_url_sha_fallback(
            &raw.source_listing_id,
            &raw_source_record_url,
        ) {
            Ok(source_listing_id) => Some(source_listing_id),
            Err(error) => {
                fail(reject(error), &mut report);
                None
            }
        },
        Err(error) => {
            fail(
                (
                    NormalizationError::SourceListingIdInvalid(
                        product_listing_core::source_listing_id::InvalidSourceListingId::Blank,
                    ),
                    NormalizationIssue::envelope(
                        NormalizationField::SourceRecordKey,
                        NormalizationIssueReason::SourceRecordKeyInvalid,
                        NormalizationIssueDetails::url_parse(&error),
                    ),
                ),
                &mut report,
            );
            None
        }
    };
    let title = normalize_title(raw.title.as_str()).and_then(|title| {
        let title_language = product_listing_normalization::detect_language(title.as_ref());
        let description_language =
            product_listing_normalization::text::detect_description_language(&raw.description);
        product_listing_normalization::text::localize_normalized_title(
            title,
            title_language,
            description_language,
        )
        .map(|localized| (localized, title_language))
    });
    let (title, title_language) = match title {
        Ok((title, title_language)) => (Some(title), title_language),
        Err(error) => {
            fail(reject(error), &mut report);
            (None, None)
        }
    };
    let description = match normalize_description(raw.description, title_language) {
        Ok(description) => Some(description),
        Err(error) => {
            fail(reject(error), &mut report);
            None
        }
    };

    let price = price_field(
        normalize_product_listing_price(raw.price.as_deref(), fallback_currency),
        PriceField::Price,
        &mut report,
        &mut fail,
    );
    let price_estimate_min = price_field(
        normalize_price(raw.price_estimate_min.as_deref(), fallback_currency),
        PriceField::EstimateMin,
        &mut report,
        &mut fail,
    );
    let price_estimate_max = price_field(
        normalize_price(raw.price_estimate_max.as_deref(), fallback_currency),
        PriceField::EstimateMax,
        &mut report,
        &mut fail,
    );
    let images = match normalize_image_urls(raw.images, &url).map_err(map_image_error) {
        Ok(images) => Some(images),
        Err(error) => {
            fail(reject(error), &mut report);
            None
        }
    };

    let outcome = match (
        first_error,
        availability,
        source_listing_id,
        title,
        description,
        price,
        price_estimate_min,
        price_estimate_max,
        images,
    ) {
        (
            None,
            Some(availability),
            Some(source_listing_id),
            Some(title),
            Some(description),
            Some(price),
            Some(price_estimate_min),
            Some(price_estimate_max),
            Some(images),
        ) => Ok(PreparedProduct {
            availability,
            source_listing_id,
            title,
            description,
            price,
            price_estimate_min,
            price_estimate_max,
            images,
            raw_attributes: raw.raw_attributes,
            raw_state: raw.state,
            url,
        }),
        (Some(error), ..) => Err(error),
        _ => unreachable!("every missing field result records its error"),
    };
    CrawlerPreparationEvaluation { outcome, report }
}

/// A supplied price without determinable currency is unresolved, not absent. Preparation does
/// not use it; raw capture keeps the supplied value so the worker preserves the canonical field.
fn price_field<T>(
    result: Result<Option<T>, PriceNormalizationError>,
    field: PriceField,
    report: &mut NormalizationReport,
    fail: &mut impl FnMut((NormalizationError, NormalizationIssue), &mut NormalizationReport),
) -> Option<Option<T>> {
    match result {
        Ok(price) => Some(price),
        Err(PriceNormalizationError::UnknownCurrency) => {
            debug!(price_field = ?field, "Price currency could not be determined");
            report.push(NormalizationIssue::price_unknown_currency(field));
            Some(None)
        }
        Err(error) => {
            let error = price_normalization_error(error, field);
            fail(reject(error), report);
            None
        }
    }
}

fn reject(error: NormalizationError) -> (NormalizationError, NormalizationIssue) {
    let issue = NormalizationIssue::from_normalization_error(&error);
    (error, issue)
}

pub struct ProductListingNormalizationServiceImpl;

impl ProductListingNormalizationServiceImpl {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for ProductListingNormalizationServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProductListingNormalizationService for ProductListingNormalizationServiceImpl {
    async fn normalize(
        &self,
        raw: RawExtractedProduct,
        context: ProductNormalizationContext,
    ) -> ProductListingNormalizationResult {
        let prepared = prepare_product(raw, context).map_err(failure)?;
        Ok(NormalizationSuccess {
            prepared,
            llm_calls_used: 0,
        })
    }
}

fn failure(error: NormalizationError) -> NormalizationFailure {
    NormalizationFailure {
        error,
        llm_calls_used: 0,
    }
}

fn map_image_error(error: ImageUrlNormalizationError) -> NormalizationError {
    match error {
        ImageUrlNormalizationError::InvalidUrl(source) => {
            NormalizationError::InvalidImageUrl(source)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_normalization::AvailabilityNormalizationError;

    fn raw() -> RawExtractedProduct {
        RawExtractedProduct {
            source_listing_id: "listing-123".into(),
            title: "Antique ceramic vase from the early twentieth century".into(),
            description: vec![],
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: "sold out".into(),
            images: vec![],
            raw_attributes: Default::default(),
        }
    }

    #[test]
    fn should_map_invalid_availability_regex_sets_to_a_system_failure() {
        let error = availability_normalization_error(
            AvailabilityNormalizationError::RegexSetCompilationFailed,
        );

        assert_eq!(
            error.failure_reason(),
            "availability_regex_set_compilation_failed"
        );
        assert_eq!(
            error.failure_scope(),
            product_listing_normalization::error::NormalizationFailureScope::System
        );
    }

    #[test]
    fn should_skip_price_when_currency_is_absent_without_a_source_fallback() {
        let mut raw = raw();
        raw.price = Some("5500".to_owned());

        let prepared = prepare_product(
            raw,
            normalization_context("https://example.com/listings/123", None),
        )
        .unwrap_or_else(|error| panic!("missing currency must not reject the product: {error}"));

        assert_eq!(None, prepared.price);
    }

    #[test]
    fn should_apply_source_fallback_when_price_has_no_currency_hint() {
        let mut raw = raw();
        raw.price = Some("5500".to_owned());

        let prepared = prepare_product(
            raw,
            normalization_context("https://example.com/listings/123", Some(Currency::Zar)),
        )
        .unwrap_or_else(|error| panic!("source fallback must normalize the price: {error}"));

        let Some(product_listing_core::product_listing_price::ProductListingPrice::Monetary(price)) =
            prepared.price
        else {
            panic!("a numeric price must be extracted with fallback currency");
        };
        assert_eq!(Currency::Zar, price.currency);
        assert_eq!(
            money::MonetaryAmount::from(550_000_u64),
            price.monetary_amount
        );
    }

    #[tokio::test]
    async fn should_use_pure_availability_normalization_without_llm_budget() {
        let result = ProductListingNormalizationServiceImpl::new()
            .normalize(
                raw(),
                normalization_context("https://example.com/listings/123", None),
            )
            .await
            .unwrap_or_else(|error| panic!("product must normalize: {error}"));

        assert_eq!(result.llm_calls_used, 0);
        assert_eq!(
            result.prepared.availability,
            ListingAvailabilityQuickCheck::Resolved(
                product_listing_core::listing_availability::ListingAvailability::SoldOut
            )
        );
    }

    #[test]
    fn should_keep_fallback_identity_based_on_the_source_record_key() {
        let mut raw = raw();
        raw.source_listing_id.clear();
        let prepared = prepare_product(
            raw,
            ProductNormalizationContext {
                effective_url: Url::parse("https://example.com/listings/new").unwrap(),
                source_record_key: "https://example.com/listings/old".to_owned(),
                fallback_currency: None,
            },
        )
        .unwrap();
        let expected = normalize_source_listing_id_with_url_sha_fallback(
            "",
            &Url::parse("https://example.com/listings/old").unwrap(),
        )
        .unwrap();
        assert_eq!(prepared.source_listing_id, expected);
    }

    fn normalization_context(
        url: &str,
        fallback_currency: Option<Currency>,
    ) -> ProductNormalizationContext {
        ProductNormalizationContext {
            effective_url: Url::parse(url)
                .unwrap_or_else(|error| panic!("test URL must parse: {error}")),
            source_record_key: url.to_owned(),
            fallback_currency,
        }
    }
}
