use regex::Regex;

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
        }
    }
}
