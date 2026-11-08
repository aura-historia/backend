//! The review console's read models, built from the crawler-local tables for presentation: the
//! review queue, the ListingSource overview and the product URLs offered to a schema
//! review. The [review repository](crate::review::repository) loads and persists reviews; these readers only read. The console builds them from that
//! repository, so they read the same database under the same review agents' decision policy.

mod listing_source_overview;
mod review_queue;
mod source_product_urls;

pub use listing_source_overview::ListingSourceOverviewReader;
pub use review_queue::ReviewQueueReader;
pub use source_product_urls::{MAX_SOURCE_PRODUCT_URLS, SourceProductUrlReader};
