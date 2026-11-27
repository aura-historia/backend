use crate::spider::classification::product_url_pattern::ProductUrlPattern;
use crate::spider::classification::url_metadata::{CrawlerDisposition, UrlClass};
use crate::spider::classification::url_metadata_repository::SpiderUrlRecord;
use std::collections::HashSet;
use std::time::Duration;

use crate::CrawlerReviewId;
use crate::logging::SourceLogContext;
use crate::spider::discovery::website_spider::CrawledPage;
use crate::spider::service::product_pattern::ProductListingPattern;
use crate::spider::utils::url::CrawledUrl;
use crate::spider::utils::url::canonical::ShopPlatform;

pub struct CrawlRunState {
    /// The ListingSource the crawl runs for, which its outcome events log.
    pub source: SourceLogContext,
    pub total_crawled: usize,
    /// How many of the crawled pages were URLs with a query string.
    pub query_url_pages: usize,
    /// How many of the crawled pages were bot challenges the shop answered instead of the page.
    pub challenged_pages: usize,
    /// The [page spelling](CrawledUrl::page_spelling) of each crawled page the product pattern
    /// matched, so a page the shop links with and without its trailing slash counts once.
    pub pattern_matched_pages: HashSet<url::Url>,
    /// The page spelling of each page the crawl stored as a product URL: a page the pattern
    /// matches unless storage keeps it `other`, such as a page an operator rejected.
    pub stored_product_pages: HashSet<url::Url>,
    /// The page spelling of each product page of `stored_product_pages` the crawl stored for the
    /// first time.
    pub new_product_pages: HashSet<url::Url>,
    /// The page spelling of each product page the shop's sitemaps listed, which the crawl
    /// stores without fetching it (`docs/crawler/networking.md#sitemaps`).
    pub sitemap_product_pages: HashSet<url::Url>,
    pub pattern: ProductListingPattern,
    pub classification_done: bool,
    pub pattern_loaded_from_store: bool,
    pub page_buffer: Vec<CrawledPage>,
    pub inference_sample: Vec<String>,
    /// Set once classification stops at a pending URL-pattern review; the crawl then keeps
    /// `pattern` as it is and asks no further, until it finds the review decided.
    pub url_pattern_review_wait: Option<UrlPatternReviewWait>,
    /// Set when the crawl ended without a URL-pattern review because it neither completed nor
    /// gathered enough URLs to open one: its URLs are stored unclassified and a later crawl adds
    /// to them.
    pub awaiting_review_sample: bool,
    /// How many URLs of `inference_sample` earlier crawls stored, preloaded before this crawl
    /// fetched a page; the crawl added the rest.
    pub preloaded_sample_urls: usize,
    /// The shop system a crawled page showed the markers of; its product URL rules then apply to
    /// every page the crawl stores.
    pub shop_platform: Option<ShopPlatform>,
    /// Time the crawl spent awaiting the storage of its crawled URLs, connection-pool waits
    /// included; it tells a run the database starved apart from one the shop's size cut short.
    pub url_store_wait: Duration,
}

/// A crawl's wait on a pending URL-pattern review.
pub struct UrlPatternReviewWait {
    pub review_id: CrawlerReviewId,
    /// The pages the crawl stored since it began to wait. The crawl's pattern matches none of
    /// them: it has none, or a stored one that matched none of its pages. A decision made while
    /// the crawl runs classifies them again.
    pub stored_pages: Vec<CrawledPage>,
}

impl CrawlRunState {
    /// Whether bot challenges are more than half of the pages crawled so far.
    pub fn mostly_bot_challenges(&self) -> bool {
        self.challenged_pages > self.total_crawled.saturating_sub(self.challenged_pages)
    }

    /// The share of the inference sample the product pattern takes; `None` without a known
    /// pattern or a sample.
    pub fn sample_match_ratio(&self) -> Option<f64> {
        let pattern = self.pattern.as_pattern()?;
        if self.inference_sample.is_empty() {
            return None;
        }
        let taken = self
            .inference_sample
            .iter()
            .filter(|sample| {
                // As the classification matches: by the page's spellings, with or without its
                // trailing slash.
                url::Url::parse(sample)
                    .is_ok_and(|url| CrawledUrl::new(url).matches_pattern(pattern))
            })
            .count();
        Some(taken as f64 / self.inference_sample.len() as f64)
    }

    /// The share of the crawled pages that were URLs with a query string.
    pub fn query_url_share(&self) -> f64 {
        match self.total_crawled {
            0 => 0.0,
            crawled => self.query_url_pages as f64 / crawled as f64,
        }
    }

    pub fn new(source: SourceLogContext, pattern: Option<ProductUrlPattern>) -> Self {
        let pattern = pattern
            .map(ProductListingPattern::from)
            .unwrap_or(ProductListingPattern::Unknown);
        let classification_done = pattern.is_known();
        let pattern_loaded_from_store = pattern.is_known();

        Self {
            source,
            total_crawled: 0,
            query_url_pages: 0,
            challenged_pages: 0,
            pattern_matched_pages: HashSet::new(),
            stored_product_pages: HashSet::new(),
            new_product_pages: HashSet::new(),
            sitemap_product_pages: HashSet::new(),
            pattern,
            classification_done,
            pattern_loaded_from_store,
            page_buffer: Vec::new(),
            inference_sample: Vec::new(),
            url_pattern_review_wait: None,
            awaiting_review_sample: false,
            preloaded_sample_urls: 0,
            shop_platform: None,
            url_store_wait: Duration::ZERO,
        }
    }

    /// How many product URLs the crawl stored, each page once.
    pub fn products_found(&self) -> usize {
        self.stored_product_pages.len()
    }

    /// How many of [`products_found`](Self::products_found) the crawl stored for the first time.
    pub fn new_products_found(&self) -> usize {
        self.new_product_pages.len()
    }

    /// Counts the product pages among `stored`, the rows a batch of the crawl's URLs was stored
    /// as; a page retired as a duplicate of another is none.
    pub fn record_stored_products(&mut self, stored: &[SpiderUrlRecord]) {
        for record in stored.iter().filter(|record| {
            record.url_class == UrlClass::ProductListing
                && record.disposition != CrawlerDisposition::Duplicate
        }) {
            let page = CrawledUrl::new(record.url.clone()).page_spelling();
            if record.newly_stored {
                self.new_product_pages.insert(page.clone());
            }
            self.stored_product_pages.insert(page);
        }
    }

    /// Counts a page the crawl fetched and adds it to the inference sample while the sample
    /// holds fewer than `max_sample_urls`.
    pub fn record_fetched_page(&mut self, page: &CrawledPage, max_sample_urls: usize) {
        self.total_crawled += 1;
        self.query_url_pages += usize::from(page.url.as_url().query().is_some());
        self.challenged_pages += usize::from(page.bot_challenge);
        self.shop_platform = self.shop_platform.or(page.identity.platform);

        if self.inference_sample.len() < max_sample_urls {
            let url = page.url.to_string();
            if !self.inference_sample.contains(&url) {
                self.inference_sample.push(url);
            }
        }
    }

    /// Counts a product page a sitemap listed. It was not fetched, so it is neither a crawled
    /// page nor part of the inference sample.
    pub fn record_sitemap_product(&mut self, page: &CrawledPage) {
        self.sitemap_product_pages.insert(page.url.page_spelling());
    }

    /// How many product URLs the crawl stored from pages it fetched that its sitemaps did not
    /// list, each page once: what the crawl found outside the sitemaps.
    pub fn products_outside_sitemaps(&self) -> usize {
        self.stored_product_pages
            .difference(&self.sitemap_product_pages)
            .count()
    }

    /// Whether the product pattern matched any page the crawl stored: what decides whether a
    /// stored pattern still fits the shop and whether the crawl found product URLs at all.
    pub fn pattern_matched_any(&self) -> bool {
        !self.pattern_matched_pages.is_empty()
    }

    /// The pending URL-pattern review the crawl waits on.
    pub fn pending_url_pattern_review(&self) -> Option<CrawlerReviewId> {
        self.url_pattern_review_wait
            .as_ref()
            .map(|wait| wait.review_id)
    }

    /// Ends the wait on the URL-pattern review with the decision made while the crawl ran: the
    /// crawl classifies with `pattern` from now on. Returns the pages it stored while waiting.
    pub fn end_url_pattern_review_wait(
        &mut self,
        pattern: ProductListingPattern,
    ) -> Vec<CrawledPage> {
        self.pattern = pattern;
        self.url_pattern_review_wait
            .take()
            .map(|wait| wait.stored_pages)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_pattern(pattern: Option<&str>) -> CrawlRunState {
        CrawlRunState::new(
            SourceLogContext::new(
                listing_source_core::ListingSourceId::new(),
                "Antique Shop",
                "antique.example",
            ),
            pattern.map(|include| {
                ProductUrlPattern::parse(include).expect("test pattern must be a valid regex")
            }),
        )
    }

    #[test]
    fn should_report_the_share_of_the_sample_the_pattern_takes() {
        let mut state = state_with_pattern(Some(r"/products/\d+$"));
        state.inference_sample = vec![
            "https://antique.example/products/1".to_owned(),
            "https://antique.example/products/2".to_owned(),
            "https://antique.example/about".to_owned(),
            "https://antique.example/contact".to_owned(),
        ];

        assert_eq!(state.sample_match_ratio(), Some(0.5));
    }

    #[test]
    fn should_count_a_trailing_slash_variant_of_a_page_the_pattern_takes() {
        let mut state = state_with_pattern(Some(r"/products/\d+/$"));
        state.inference_sample = vec![
            "https://antique.example/products/1".to_owned(),
            "https://antique.example/about".to_owned(),
        ];

        assert_eq!(state.sample_match_ratio(), Some(0.5));
    }

    #[test]
    fn should_report_no_match_ratio_without_a_pattern_or_a_sample() {
        let mut without_pattern = state_with_pattern(None);
        without_pattern.inference_sample = vec!["https://antique.example/a".to_owned()];
        let without_sample = state_with_pattern(Some("/products/"));

        assert_eq!(without_pattern.sample_match_ratio(), None);
        assert_eq!(without_sample.sample_match_ratio(), None);
    }

    fn page(path: &str) -> CrawledPage {
        CrawledPage::new(CrawledUrl::new(
            url::Url::parse(&format!("https://antique.example{path}")).unwrap(),
        ))
    }

    #[test]
    fn should_count_the_stored_products_the_sitemaps_did_not_list() {
        let mut state = state_with_pattern(Some("/products/"));
        state.record_sitemap_product(&CrawledPage::listed_in_sitemap(page("/products/1/").url));
        state.record_fetched_page(&page("/products/2"), 10);
        state.stored_product_pages = ["/products/1", "/products/2", "/products/3"]
            .into_iter()
            .map(|path| page(path).url.page_spelling())
            .collect();

        assert_eq!(state.products_outside_sitemaps(), 2);
        assert_eq!(state.total_crawled, 1);
        assert_eq!(
            state.inference_sample,
            ["https://antique.example/products/2"]
        );
    }

    fn stored(path: &str, url_class: UrlClass, newly_stored: bool) -> SpiderUrlRecord {
        let now = time::OffsetDateTime::now_utc();
        SpiderUrlRecord {
            listing_source_id: listing_source_core::ListingSourceId::new(),
            url: page(path).url.as_url().clone(),
            url_class,
            disposition: CrawlerDisposition::Active,
            last_scraped_hash: None,
            last_scraped: None,
            created: now,
            updated: now,
            newly_stored,
        }
    }

    #[test]
    fn should_count_as_new_only_the_product_pages_stored_for_the_first_time() {
        let mut state = state_with_pattern(Some("/products/"));
        state.record_stored_products(&[
            stored("/products/1", UrlClass::ProductListing, true),
            stored("/products/2", UrlClass::ProductListing, false),
            stored("/about", UrlClass::Other, true),
        ]);
        state.record_stored_products(&[stored("/products/1/", UrlClass::ProductListing, false)]);

        assert_eq!((state.products_found(), state.new_products_found()), (2, 1));
    }

    #[test]
    fn should_report_the_share_of_crawled_pages_with_a_query_string() {
        let mut state = state_with_pattern(None);
        assert_eq!(state.query_url_share(), 0.0);

        state.total_crawled = 4;
        state.query_url_pages = 3;

        assert_eq!(state.query_url_share(), 0.75);
    }
}
