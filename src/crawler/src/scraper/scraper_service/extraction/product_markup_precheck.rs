//! The deterministic check a page that fails every live product schema passes before it opens a
//! product-schema review: on a ListingSource whose product pages reliably carry product markup,
//! a page whose markup shows a listing of the source's products is classified as no product page
//! instead (`docs/crawler/extraction-and-review.md#product-markup-pre-check`).

use crate::logging::{SourceLogContext, warn_in_source};
use crate::scraper::product_markup::ProductMarkup;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::pipeline::fresh_schema_generation::FreshSchemaGenerationContext;
use crate::scraper::scraper_service::pipeline::scrape_product::is_same_logical_host;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use crate::spider::classification::product_url_pattern::ProductUrlPattern;
use crate::spider::classification::url_metadata::{CrawlerUrlWriteOutcome, UrlClass};
use crate::spider::utils::url::CrawledUrl;
use tracing::{debug, info};
use url::Url;

/// The outcome event of a page the pre-check classified (`docs/crawler/operations.md#outcome-events`).
const CLASSIFIED_BY_PRODUCT_MARKUP_EVENT: &str = "crawler.scraper.classified_by_product_markup";

impl ScraperServiceImpl {
    /// Classifies the requested crawler URL as other and fails with
    /// [`ScraperError::NotProductPageByMarkup`] when its page's markup
    /// [shows a listing](ProductMarkup::shows_a_listing) of the ListingSource's products while its
    /// product pages reliably name one product each; `Ok` leaves the page to its review. Only a
    /// page requested as itself whose URL never captured a product is judged: a redirect's target,
    /// or a URL that once captured one, may show a removal the markup cannot tell from a listing.
    ///
    /// The check never fails the scrape: an evidence read or a classification that fails, and a
    /// classification the row's guard refuses because the row changed since the scrape read it,
    /// classify nothing, and the page goes to its review as if the check had not run.
    pub(crate) async fn classify_by_product_markup(
        &self,
        ctx: &FreshSchemaGenerationContext<'_>,
    ) -> Result<(), ScraperError> {
        let never_captured = ctx.expected_last_captured_raw_input_sha256.is_none()
            && ctx.last_source_listing_id.is_none();
        if ctx.requested_url != ctx.url || !never_captured {
            return Ok(());
        }
        let is_product_url =
            |listed: &str| listed_product_url(ctx.url, ctx.product_url_pattern, listed);
        if !ProductMarkup::of_page(ctx.html, &is_product_url).shows_a_listing() {
            return Ok(());
        }
        let evidence = match self
            .candidate_service
            .product_markup_evidence(ctx.listing_source_id)
            .await
        {
            Ok(evidence) => evidence,
            Err(error) => {
                warn_in_source!(
                    error = %error,
                    "Failed to read the product markup evidence; leaving the page to its review"
                );
                return Ok(());
            }
        };
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
            "Classified a page whose markup shows a listing of products as no product page instead of opening a product-schema review"
        );
        Err(ScraperError::NotProductPageByMarkup {
            url: ctx.requested_url.clone(),
            pages_with_verdict: evidence.pages_with_verdict,
        })
    }
}

/// Whether a URL a list on the page at `page_url` names is a product URL of its ListingSource: on
/// the page's host, another page than it, and taken by the live product URL `pattern`. Without a
/// pattern no listed URL is a product URL.
fn listed_product_url(page_url: &Url, pattern: Option<&ProductUrlPattern>, listed: &str) -> bool {
    let Some(pattern) = pattern else {
        return false;
    };
    let Ok(listed) = page_url.join(listed) else {
        return false;
    };
    if !is_same_logical_host(&listed, page_url) {
        return false;
    }
    let listed = CrawledUrl::new(listed);
    !listed.is_same_page(&CrawledUrl::new(page_url.clone())) && listed.matches_pattern(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> Url {
        Url::parse("https://shop.example/rings/").unwrap()
    }

    #[test]
    fn should_take_a_listed_url_the_product_pattern_takes() {
        let pattern = ProductUrlPattern::parse("/products/").unwrap();
        for listed in [
            "https://shop.example/products/ring-a",
            "/products/ring-a",
            "https://www.shop.example/products/ring-a",
        ] {
            assert!(
                listed_product_url(&page(), Some(&pattern), listed),
                "{listed}"
            );
        }
    }

    #[test]
    fn should_refuse_a_listed_url_outside_the_pattern_the_host_or_the_page() {
        let pattern = ProductUrlPattern::parse("/(?:products|rings)/").unwrap();
        for listed in [
            "https://shop.example/pages/about",
            "https://other.example/products/ring-a",
            "https://shop.example/rings",
            "https://[unclosed/products/a",
        ] {
            assert!(
                !listed_product_url(&page(), Some(&pattern), listed),
                "{listed}"
            );
        }
        assert!(!listed_product_url(
            &page(),
            None,
            "https://shop.example/products/ring-a"
        ));
    }
}
