//! The deterministic check a page that fails every live product schema passes before it opens a
//! product-schema review: on a ListingSource whose product pages reliably carry product markup,
//! a page whose markup shows no product of its own, or several, is classified as no product page
//! instead (`docs/crawler/extraction-and-review.md#product-markup-pre-check`).

use crate::logging::{SourceLogContext, warn_in_source};
use crate::scraper::product_markup::ProductMarkup;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::pipeline::fresh_schema_generation::FreshSchemaGenerationContext;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use crate::spider::classification::url_metadata::{CrawlerUrlWriteOutcome, UrlClass};
use tracing::{debug, info};

/// The outcome event of a page the pre-check classified (`docs/crawler/operations.md#outcome-events`).
const CLASSIFIED_BY_PRODUCT_MARKUP_EVENT: &str = "crawler.scraper.classified_by_product_markup";

impl ScraperServiceImpl {
    /// Classifies the requested crawler URL as other and fails with
    /// [`ScraperError::NotProductPageByMarkup`] when its page's markup
    /// [shows a listing](ProductMarkup::shows_a_listing) while the ListingSource's
    /// product pages reliably name one product each; `Ok` leaves the page to its review. Only a
    /// page requested as itself whose URL never captured a product is judged: a redirect's target,
    /// or a URL that once captured one, may show a removal the markup cannot tell from a
    /// non-product page.
    ///
    /// The evidence read and the classification are two statements, as the classification is the
    /// guarded single-row write every scrape makes. A classification that is not written, because
    /// the row changed since the scrape read it or the write failed, classifies nothing: the page
    /// goes to its review as if the check had not run.
    pub(crate) async fn classify_by_product_markup(
        &self,
        ctx: &FreshSchemaGenerationContext<'_>,
    ) -> Result<(), ScraperError> {
        let never_captured = ctx.expected_last_captured_raw_input_sha256.is_none()
            && ctx.last_source_listing_id.is_none();
        if ctx.requested_url != ctx.url
            || !never_captured
            || !ProductMarkup::of_page(ctx.html).shows_a_listing()
        {
            return Ok(());
        }
        let evidence = self
            .candidate_service
            .product_markup_evidence(ctx.listing_source_id)
            .await
            .map_err(ScraperError::ProductMarkupEvidenceLookup)?;
        if !evidence.marks_products_reliably() {
            return Ok(());
        }
        match self
            .candidate_service
            .set_class(
                ctx.listing_source_id,
                ctx.requested_url,
                UrlClass::Other,
                None,
            )
            .await
        {
            Ok(CrawlerUrlWriteOutcome::Applied) => {}
            Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                debug!(
                    crawler_url_write_outcome = "stale_noop",
                    "The page's row changed since the scrape read it; leaving the page to its review instead of classifying it by its markup"
                );
                return Ok(());
            }
            Err(error) => {
                warn_in_source!(
                    error = %error,
                    "Failed to classify a page by its product markup; leaving it to its review"
                );
                return Ok(());
            }
        }
        let source = SourceLogContext::current();
        info!(
            event = CLASSIFIED_BY_PRODUCT_MARKUP_EVENT,
            listing_source_id = %ctx.listing_source_id,
            listing_source_name = source.as_ref().map(|source| source.listing_source_name.as_str()),
            domain = ctx.domain,
            url = %ctx.requested_url,
            pages_with_verdict = evidence.pages_with_verdict,
            "Classified a page without a product of its own as no product page instead of opening a product-schema review"
        );
        Err(ScraperError::NotProductPageByMarkup {
            url: ctx.requested_url.clone(),
            pages_with_verdict: evidence.pages_with_verdict,
        })
    }
}
