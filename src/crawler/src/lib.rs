// The scraper's `async` call chain nests deeper than the default limit of 128 once the release
// build optimizes it: computing the layout of the future in `ScraperService::scrape` failed with
// "queries overflow the depth limit". The test profile never reaches that depth.
#![recursion_limit = "256"]

pub mod authoring;
pub mod crawl_root;
pub mod llm_runtime;
pub mod local_db;
pub mod logging;
pub mod metrics;
pub mod network;
pub mod object_id;
pub mod review;
pub mod review_agent_runner;
pub mod review_mcp;
pub mod runtime;
pub mod scraper;
pub mod service;
pub mod spider;
#[cfg(test)]
mod test_support;
pub mod vertex_ai;

pub use crawl_root::CrawlRoot;
pub use object_id::{
    CrawlerReviewEventId, CrawlerReviewId, CrawlerReviewPageId, CrawlerReviewUrlId,
};
