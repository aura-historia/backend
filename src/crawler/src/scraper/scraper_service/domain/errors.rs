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

    /// The product URL redirects to a product URL its ListingSource stores under its own row, so
    /// the URL is that page's duplicate (`docs/crawler/networking.md#duplicate-targets`).
    #[error("redirects to '{destination}', a product URL of its ListingSource")]
    RedirectToKnownProduct { destination: Url },

    /// The rows stored at the redirect target do not settle whether the URL is a duplicate, so it
    /// is neither retired nor removed.
    #[error(
        "redirects to '{destination}', whose stored rows do not settle whether it is a duplicate"
    )]
    RedirectDestinationUnsettled { destination: Url },

    /// The rows stored at a redirect target could not be read.
    #[error("Redirect destination lookup failed: {0}")]
    RedirectDestinationLookup(#[source] sqlx::Error),

    /// The product URL redirects outside its product URL pattern and no cached schema can show
    /// whether the target is a product page, so it is neither removed nor taken as a schema
    /// review's page.
    #[error(
        "redirects outside its product URL pattern to '{destination}' and no cached schema judges that page"
    )]
    RedirectTargetUnverified { destination: Url },

    #[error("Rejected low-confidence page classification for '{url}': {details}")]
    SchemaClassificationRejected { url: Url, details: String },

    #[error("URL has no host: {url}")]
    NoHost { url: Url },

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

    /// The target of a redirect that neither retires nor removes the URL: one whose stored rows
    /// disagree, or one outside the product URL pattern that no cached schema judges.
    pub(crate) fn unsettled_redirect_destination(&self) -> Option<&Url> {
        match self {
            Self::RedirectDestinationUnsettled { destination }
            | Self::RedirectTargetUnverified { destination } => Some(destination),
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
    /// What showed the removal, which the capture of the removal logs.
    pub evidence: RemovalEvidence,
    /// The stored removed-page templates whose match withdrew the page; empty for a removal the
    /// shop's response showed. A template retired or narrowed later re-arms the page.
    pub removed_by_templates: Vec<RemovedPageSchema>,
}

impl ListingRemoval {
    /// The shared redirect target whose sharing showed the removal, which the URL records with
    /// it so the evidence outlives the removal write; `None` for any other evidence.
    pub fn removed_by_redirect_target(&self) -> Option<&Url> {
        match self.evidence.kind {
            RemovalEvidenceKind::RedirectToSharedPage => self.evidence.final_url.as_ref(),
            _ => None,
        }
    }
}

impl ScraperError {
    /// A removal of the listing at `url` that `evidence` showed, which `removed_by_templates`
    /// withdrew when any.
    pub fn product_listing_removed(
        url: Url,
        details: impl Into<String>,
        evidence: RemovalEvidence,
        removed_by_templates: Vec<RemovedPageSchema>,
    ) -> Self {
        Self::ProductListingRemoved(Box::new(ListingRemoval {
            url,
            details: details.into(),
            evidence,
            removed_by_templates,
        }))
    }
}

/// What showed that a listing is removed: the shop's answer, or the page a redirect led to or a
/// removed-page template matched (`docs/crawler/networking.md#redirect-removal-evidence` names
/// what a redirect must show).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovalEvidence {
    pub kind: RemovalEvidenceKind,
    /// The status the shop answered: 404 or 410, or the success status of the page the evidence
    /// was read from. `None` when that page was not fetched or its status is not known here.
    pub status: Option<u16>,
    /// The page the evidence was read from; `None` for a 404 or 410 answer.
    pub final_url: Option<Url>,
}

impl RemovalEvidence {
    /// The shop answered the product URL with `status`, 404 or 410.
    pub fn gone_status(status: u16) -> Self {
        Self {
            kind: RemovalEvidenceKind::GoneStatus,
            status: Some(status),
            final_url: None,
        }
    }

    /// `kind` read from the page at `final_url`, answered with `status` when known.
    pub fn page(kind: RemovalEvidenceKind, final_url: Url, status: Option<u16>) -> Self {
        Self {
            kind,
            status,
            final_url: Some(final_url),
        }
    }
}

/// The kinds of [`RemovalEvidence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalEvidenceKind {
    /// The shop answered the product URL with 404 or 410.
    GoneStatus,
    /// The product URL redirected to the shop's homepage.
    RedirectToHomepage,
    /// The product URL redirected outside its product URL pattern to a page every cached schema
    /// fails on.
    RedirectToNonProductPage,
    /// The product URL redirected to a page the crawler stores as no product page of its
    /// ListingSource.
    RedirectToStoredNonProductPage,
    /// The product URL redirected to a page an operator rejected as no product page.
    RedirectToRejectedPage,
    /// The product URL redirected outside its product URL pattern to a page robots.txt disallows,
    /// which is judged by its URL alone.
    RedirectToDisallowedPage,
    /// The product URL keeps redirecting outside its product URL pattern to a page that many
    /// other product URLs of its ListingSource redirect to as well: a catch-all page, such as an
    /// error page, stands for none of the products.
    RedirectToSharedPage,
    /// A stored removed-page template matched the page.
    RemovedPageTemplate,
    /// Fresh schema generation classified the page as removed.
    RemovedPageClassification,
}

impl RemovalEvidenceKind {
    /// The kind's name in log lines.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GoneStatus => "GONE_STATUS",
            Self::RedirectToHomepage => "REDIRECT_TO_HOMEPAGE",
            Self::RedirectToNonProductPage => "REDIRECT_TO_NON_PRODUCT_PAGE",
            Self::RedirectToStoredNonProductPage => "REDIRECT_TO_STORED_NON_PRODUCT_PAGE",
            Self::RedirectToRejectedPage => "REDIRECT_TO_REJECTED_PAGE",
            Self::RedirectToDisallowedPage => "REDIRECT_TO_DISALLOWED_PAGE",
            Self::RedirectToSharedPage => "REDIRECT_TO_SHARED_PAGE",
            Self::RemovedPageTemplate => "REMOVED_PAGE_TEMPLATE",
            Self::RemovedPageClassification => "REMOVED_PAGE_CLASSIFICATION",
        }
    }
}
