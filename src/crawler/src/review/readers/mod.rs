//! The review console's read models, built from the crawler-local tables for presentation: the
//! review queue and the ListingSource overview. The [review repository](crate::review::repository)
//! loads and persists reviews; these readers only read. The console builds them from that
//! repository, so they read the same database under the same review agents' decision policy.

mod listing_source_overview;
mod review_queue;

pub use listing_source_overview::ListingSourceOverviewReader;
pub use review_queue::ReviewQueueReader;
