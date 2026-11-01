mod listing_sources_removed;
mod page_match;
mod removed;
mod verdict;

pub use listing_sources_removed::ListingSourceRemovedPageSchema;
pub use removed::{
    RemovedPageMatch, RemovedPageSchema, extend_with_new_removed_page_schemas,
    rebase_reviewed_removed_page_schemas,
};
pub use verdict::{RemovedPageVerdict, match_removed_page, removed_page_verdict};
