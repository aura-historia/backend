pub(crate) mod auto_throttle;
pub mod domain;
pub(crate) mod extraction;
pub(crate) mod image_validation;
pub(crate) mod pipeline;
pub(crate) mod recovery;
pub mod service;

pub use extraction::schema_candidates::rank_applicable_schema_indices;
pub(crate) mod util;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Public re-exports — all external callers continue to use
// `crate::scraper::scraper_service::Foo` unchanged.
// ---------------------------------------------------------------------------

pub use auto_throttle::{ScraperAutoThrottle, ScraperAutoThrottleConfig};
pub use domain::errors::ScraperError;
pub use domain::product::{MockScraperService, ScrapedProduct, ScraperService};
pub use service::{
    DEFAULT_MAX_LLM_CALLS_PER_SHOP, DEFAULT_SCHEMA_SEED_PAGES, FetchError, FetchedHtml,
    HtmlFetcher, MockHtmlFetcher, ReqwestHtmlFetcher, SchemaLlmReviewMode, ScraperServiceImpl,
};
