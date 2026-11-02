use crate::CrawlerReviewId;
use crate::network::policy::NetworkErrorKind;
use crate::scraper::css_selector::product_schema::ApplySchemaError;
use crate::scraper::css_selector::product_schema_service::ProductListingSchemaServiceError;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::plausibility::PlausibilityFindings;
use listing_source_core::ListingSourceId;
use product_listing_normalization::{NormalizationError, NormalizationInputError};
use std::fmt;
use url::Url;

#[derive(Debug)]
pub struct HttpErrorMetadata {
    pub url: Url,
    pub kind: NetworkErrorKind,
    pub status_code: Option<u16>,
    pub retry_after: Option<std::time::Duration>,
    pub details: String,
}

impl fmt::Display for HttpErrorMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "HTTP error while fetching '{}': {}",
            self.url, self.details
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScraperError {
    #[error("HTTP error while fetching '{url}': {details}")]
    HttpError {
        url: Url,
        kind: NetworkErrorKind,
        details: String,
    },

    #[error("{0}")]
    HttpErrorWithMetadata(Box<HttpErrorMetadata>),

    #[error("ProductListing URL removed while fetching '{}': {}", .0.url, .0.details)]
    ProductListingRemoved(Box<ListingRemoval>),

    #[error("URL is not a product page while scraping '{url}': {details}")]
    NotProductPage { url: Url, details: String },

    #[error("Rejected low-confidence page classification for '{url}': {details}")]
    SchemaClassificationRejected { url: Url, details: String },

    #[error("URL has no host: {url}")]
    NoHost { url: Url },

    #[error("Persisted crawler domain context is required for {mode} scraping of '{url}'")]
    MissingDomainContext { url: Url, mode: &'static str },

    #[error("Schema service error: {0}")]
    SchemaServiceError(#[from] ProductListingSchemaServiceError),

    #[error("Removed-page schema database error: {0}")]
    RemovedPageSchemaDatabaseError(#[source] sqlx::Error),

    #[error(
        "Schema regeneration exhausted after {attempts} attempts for '{url}' (last error: {last_error})"
    )]
    SchemaRegenerationExhausted {
        url: Url,
        attempts: u32,
        last_error: Box<ApplySchemaError>,
    },

    /// Fresh schema generation succeeded, but its extraction of the page failed a
    /// rejecting plausibility check.
    #[error("Freshly generated schema extracted implausible values from '{url}': {findings}")]
    FreshSchemaImplausible {
        url: Url,
        findings: PlausibilityFindings,
    },

    /// A cached schema found the element of a required value, but the page leaves it empty,
    /// so the page has nothing to capture and needs no schema work. `findings` names the
    /// empty fields.
    #[error("Required value missing from '{url}': {findings}")]
    MissingRequiredValue {
        url: Url,
        findings: PlausibilityFindings,
    },

    /// The shared kernel could not derive the captured source listing id.
    #[error("Normalization error: {0}")]
    NormalizationError(#[from] NormalizationError),

    #[error("crawler raw normalization input could not be built")]
    RawNormalizationInput(#[source] NormalizationInputError),

    #[error("crawler effective schema fingerprint could not be built")]
    SchemaFingerprint(#[source] serde_json::Error),

    #[error(
        "LLM call budget exceeded for ListingSource '{listing_source_id}' while scraping '{url}' (limit={max_calls})"
    )]
    LlmBudgetExceeded {
        listing_source_id: ListingSourceId,
        url: Url,
        max_calls: i64,
    },

    #[error("Scraping '{url}' is blocked pending product schema review '{review_id}'")]
    PendingSchemaReview {
        url: Url,
        review_id: CrawlerReviewId,
    },
}

impl ScraperError {
    /// The network failure kind of an HTTP fetch error.
    pub(crate) fn network_error_kind(&self) -> Option<NetworkErrorKind> {
        match self {
            Self::HttpError { kind, .. } => Some(*kind),
            Self::HttpErrorWithMetadata(metadata) => Some(metadata.kind),
            _ => None,
        }
    }
}

/// A verified removal of a product listing, with the evidence a later configuration change
/// weighs.
#[derive(Debug)]
pub struct ListingRemoval {
    pub url: Url,
    pub details: String,
    /// The stored removed-page templates whose match withdrew the page; empty for a removal the
    /// shop's response showed. A template retired or narrowed later re-arms the page.
    pub removed_by_templates: Vec<RemovedPageSchema>,
}

impl ScraperError {
    /// A removal of the listing at `url`, which `removed_by_templates` withdrew when any.
    pub fn product_listing_removed(
        url: Url,
        details: impl Into<String>,
        removed_by_templates: Vec<RemovedPageSchema>,
    ) -> Self {
        Self::ProductListingRemoved(Box::new(ListingRemoval {
            url,
            details: details.into(),
            removed_by_templates,
        }))
    }
}
