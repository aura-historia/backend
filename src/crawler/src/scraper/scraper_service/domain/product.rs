use crate::network::policy::{DomainFailureKind, domain_failure_kind};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use listing_source_core::ListingSourceId;
use money::Currency;
use product_listing_normalization::{
    ListingAvailabilityQuickCheck, ProductListingNormalizationInput,
};
use url::Url;

/// Result of a successful scrape — the raw normalization input together with
/// metadata needed to mark the URL as scraped after durable raw capture.
#[derive(Debug)]
pub struct ScrapedProduct {
    /// Complete source-neutral normalization input. The worker later performs canonical writes.
    pub raw_input: ProductListingNormalizationInput,
    /// Pure crawler quick-check used only for local disposition.
    pub availability: ListingAvailabilityQuickCheck,
    /// SHA-256 of the page's `<main>` fragment (or full HTML) that was used to
    /// detect whether the page had changed.
    pub hash: String,
    /// Deterministic fingerprint of the effective ordered schema set.
    pub schema_fingerprint: String,
    /// Shared provider-neutral raw-input hash used for local change detection.
    pub raw_input_sha256: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrapeMode {
    Normal,
    DomainProbe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainFetchHealth {
    NotObserved,
    Responsive,
    CircuitFailure {
        kind: DomainFailureKind,
        status_code: Option<u16>,
        retry_after: Option<std::time::Duration>,
    },
}

impl Default for DomainFetchHealth {
    fn default() -> Self {
        Self::NotObserved
    }
}

pub struct ScrapeOutcome {
    pub result: Result<Option<ScrapedProduct>, ScraperError>,
    pub domain_health: DomainFetchHealth,
}

pub(crate) fn domain_health_for_scraper_error(error: &ScraperError) -> DomainFetchHealth {
    match error {
        ScraperError::HttpError { kind, .. } => match domain_failure_kind(*kind) {
            Some(kind) => DomainFetchHealth::CircuitFailure {
                kind,
                status_code: match kind {
                    DomainFailureKind::Http408 => Some(408),
                    DomainFailureKind::Http429 => Some(429),
                    DomainFailureKind::Http503 => Some(503),
                    DomainFailureKind::Http504 => Some(504),
                    _ => None,
                },
                retry_after: None,
            },
            None => DomainFetchHealth::Responsive,
        },
        ScraperError::HttpErrorWithMetadata {
            kind,
            status_code,
            retry_after,
            ..
        } => match domain_failure_kind(*kind) {
            Some(kind) => DomainFetchHealth::CircuitFailure {
                kind,
                status_code: *status_code,
                retry_after: *retry_after,
            },
            None => DomainFetchHealth::Responsive,
        },
        ScraperError::ProductListingRemoved { .. }
        | ScraperError::NotProductPage { .. }
        | ScraperError::SchemaClassificationRejected { .. }
        | ScraperError::SchemaServiceError(_)
        | ScraperError::RemovedPageSchemaDatabaseError(_)
        | ScraperError::SchemaRegenerationExhausted { .. }
        | ScraperError::FreshSchemaNormalizationFailed { .. }
        | ScraperError::NormalizationError(_)
        | ScraperError::RawNormalizationInput(_)
        | ScraperError::SchemaFingerprint(_)
        | ScraperError::LlmBudgetExceeded { .. } => DomainFetchHealth::Responsive,
        ScraperError::NoHost { .. } | ScraperError::PendingSchemaReview { .. } => {
            DomainFetchHealth::NotObserved
        }
    }
}

// ---------------------------------------------------------------------------
// ScraperService trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
#[mockall::automock]
pub trait ScraperService: Send + Sync {
    /// Fetch the product page at `url`, extract structured data using the CSS
    /// selector schema for `listing_source_id`, validate a plausible extraction, and return a
    /// [`ScrapedProduct`]. The caller captures it before calling
    /// [`crate::scraper::candidate_service::ScraperCandidateService::mark_as_scraped`].
    async fn scrape(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        product_url_pattern: Option<&str>,
        last_scraped_hash: Option<&str>,
        last_scraped_schema_fingerprint: Option<&str>,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<Option<ScrapedProduct>, ScraperError>;

    #[allow(clippy::too_many_arguments)]
    async fn scrape_with_fallback_currency(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        product_url_pattern: Option<&str>,
        last_scraped_hash: Option<&str>,
        last_scraped_schema_fingerprint: Option<&str>,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
        fallback_currency: Option<Currency>,
    ) -> Result<Option<ScrapedProduct>, ScraperError> {
        let _ = fallback_currency;
        self.scrape(
            listing_source_id,
            url,
            product_url_pattern,
            last_scraped_hash,
            last_scraped_schema_fingerprint,
            expected_last_captured_raw_input_sha256,
        )
        .await
    }

    /// Executes one scrape while reporting the independent transport health
    /// observation used by the domain circuit. The default keeps existing
    /// implementations and test doubles compatible.
    #[allow(clippy::too_many_arguments)]
    async fn scrape_with_mode(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        product_url_pattern: Option<&str>,
        last_scraped_hash: Option<&str>,
        last_scraped_schema_fingerprint: Option<&str>,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
        fallback_currency: Option<Currency>,
        _mode: ScrapeMode,
    ) -> ScrapeOutcome {
        let result = self
            .scrape_with_fallback_currency(
                listing_source_id,
                url,
                product_url_pattern,
                last_scraped_hash,
                last_scraped_schema_fingerprint,
                expected_last_captured_raw_input_sha256,
                fallback_currency,
            )
            .await;
        let domain_health = match &result {
            Ok(_) => DomainFetchHealth::Responsive,
            Err(error) => domain_health_for_scraper_error(error),
        };
        ScrapeOutcome {
            result,
            domain_health,
        }
    }
}
