//! The review console's read models, built from the crawler-local tables for presentation: the
//! review queue, the ListingSource overview and page, the counts of the first page and the product
//! URLs offered to a schema review. The [review repository](crate::review::repository) loads and
//! persists reviews; these readers only read. The console builds them from that repository, so
//! they read the same database under the same review agents' decision policy.

mod console_overview;
mod listing_source_detail;
mod listing_source_overview;
mod review_queue;
mod source_product_urls;

pub use console_overview::ConsoleOverviewReader;
pub use listing_source_detail::ListingSourceDetailReader;
pub use listing_source_overview::{HealthCounts, ListingSourceOverviewReader, ListingSourceQuery};
pub use review_queue::{ReviewQuery, ReviewQueueReader};
pub use source_product_urls::{MAX_SOURCE_PRODUCT_URLS, SourceProductUrlReader};
