pub mod domain;
pub(crate) mod extraction;
pub(crate) mod pipeline;
pub(crate) mod recovery;
pub mod service;
pub(crate) mod util;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Public re-exports — all external callers continue to use
// `crate::scraper::scraper_service::Foo` unchanged.
// ---------------------------------------------------------------------------

pub use domain::errors::ScraperError;
pub use domain::product::{MockScraperService, ScrapedProduct, ScraperService};
pub use service::{
    DEFAULT_MAX_LLM_CALLS_PER_SHOP, DEFAULT_SCHEMA_SEED_PAGES, FetchError, HtmlFetcher,
    MockHtmlFetcher, ReqwestHtmlFetcher, SchemaLlmReviewMode, ScraperServiceImpl,
};
