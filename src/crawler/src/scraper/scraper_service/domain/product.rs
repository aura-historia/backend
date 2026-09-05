use crate::scraper::normalization::product::NormalizedProduct;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use listing_source_core::ListingSourceId;
use url::Url;

/// Result of a successful scrape — the normalized product together with the
/// metadata needed to mark the URL as scraped *after* the push has been
/// confirmed.
#[derive(Debug)]
pub struct ScrapedProduct {
    pub product: NormalizedProduct,
    /// SHA-256 of the page's `<main>` fragment (or full HTML) that was used to
    /// detect whether the page had changed.
    pub hash: String,
    /// Deterministic fingerprint of the effective ordered schema set.
    pub schema_fingerprint: String,
    /// Shared provider-neutral raw-input hash used for local change detection.
    pub raw_input_sha256: Vec<u8>,
}

// ---------------------------------------------------------------------------
// ScraperService trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
#[mockall::automock]
pub trait ScraperService: Send + Sync {
    /// Fetch the product page at `url`, extract structured data using the CSS
    /// selector schema for `listing_source_id`, normalise the raw data, and return a
    /// [`ScrapedProduct`].  The caller is responsible for calling
    /// [`crate::scraper::candidate_service::ScraperCandidateService::mark_as_scraped`]
    /// once the product has been successfully pushed to the backend.
    async fn scrape(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        product_url_pattern: Option<&str>,
        last_scraped_hash: Option<&str>,
        last_scraped_schema_fingerprint: Option<&str>,
    ) -> Result<Option<ScrapedProduct>, ScraperError>;
}
