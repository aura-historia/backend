//! The deterministic check a page that fails every live product schema passes before it opens a
//! product-schema review: on a ListingSource whose product pages reliably carry schema.org
//! `Product` markup, a page without exactly one such Product is classified as no product page
//! instead (`docs/crawler/extraction-and-review.md#product-markup-pre-check`).

use crate::review::product_markup::describes_one_product;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::pipeline::fresh_schema_generation::FreshSchemaGenerationContext;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use tracing::info;

/// The outcome event of a page the pre-check classified (`docs/crawler/operations.md#outcome-events`).
const CLASSIFIED_BY_PRODUCT_MARKUP_EVENT: &str = "crawler.scraper.classified_by_product_markup";

impl ScraperServiceImpl {
    /// Classifies the requested crawler URL as other and fails with
    /// [`ScraperError::NotProductPage`] when its page does not describe exactly one schema.org
    /// `Product` while the ListingSource's product pages reliably do, as an approved no-product
    /// mark would; `Ok` leaves the page to its review. Only a page requested as itself and never
    /// captured is judged: a redirect's target, or a URL that once captured a product, may be a
    /// removal that the markup cannot tell from a non-product page.
    ///
    /// The evidence read and the classification are two statements, as the classification is the
    /// guarded single-row write every scrape makes ([`Self::mark_url_other_best_effort`]); a page
    /// whose write is skipped is judged again on its next scrape.
    pub(crate) async fn classify_by_product_markup(
        &self,
        ctx: &FreshSchemaGenerationContext<'_>,
    ) -> Result<(), ScraperError> {
        if ctx.requested_url != ctx.url
            || ctx.expected_last_captured_raw_input_sha256.is_some()
            || describes_one_product(ctx.html)
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
        self.mark_url_other_best_effort(ctx.listing_source_id, ctx.requested_url, None)
            .await;
        info!(
            event = CLASSIFIED_BY_PRODUCT_MARKUP_EVENT,
            listing_source_id = %ctx.listing_source_id,
            url = %ctx.requested_url,
            pages_with_verdict = evidence.pages_with_verdict,
            "Classified a page without one schema.org Product as no product page instead of opening a product-schema review"
        );
        Err(ScraperError::NotProductPage {
            url: ctx.requested_url.clone(),
            details: format!(
                "the page does not describe exactly one schema.org Product, while all {} product pages of its ListingSource with a recorded verdict do",
                evidence.pages_with_verdict
            ),
        })
    }
}
