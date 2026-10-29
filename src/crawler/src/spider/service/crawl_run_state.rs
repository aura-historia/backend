use regex::Regex;

use crate::CrawlerReviewId;
use crate::spider::discovery::website_spider::CrawledPage;
use crate::spider::service::product_pattern::ProductListingPattern;

pub struct CrawlRunState {
    pub total_crawled: usize,
    pub products_found: usize,
    pub pattern: ProductListingPattern,
    pub classification_done: bool,
    pub pattern_loaded_from_store: bool,
    pub page_buffer: Vec<CrawledPage>,
    pub inference_sample: Vec<String>,
    /// Set once classification stops at a pending URL-pattern review; the crawl then keeps
    /// `pattern` as it is and asks no further, until it finds the review decided.
    pub url_pattern_review_wait: Option<UrlPatternReviewWait>,
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
    pub fn new(pattern: Option<Regex>) -> Self {
        let pattern = pattern
            .map(ProductListingPattern::from)
            .unwrap_or(ProductListingPattern::Unknown);
        let classification_done = pattern.is_known();
        let pattern_loaded_from_store = pattern.is_known();

        Self {
            total_crawled: 0,
            products_found: 0,
            pattern,
            classification_done,
            pattern_loaded_from_store,
            page_buffer: Vec::new(),
            inference_sample: Vec::new(),
            url_pattern_review_wait: None,
        }
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
