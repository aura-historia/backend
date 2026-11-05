//! The crawl root a spider run crawled. A scope refresh that gives a ListingSource another
//! presentation URL moves the source to its new crawl root and clears the old root's crawl state,
//! while a run of the old root may still be crawling (`docs/crawler/README.md#listingsource-scope`).

use listing_source_core::ListingSourceId;

/// The ListingSource a spider run crawled and the crawl root URL it crawled, as the source had it
/// when the run started. Every write of the run's outcome to the source (its crawl state, frontier
/// and last full crawl) compares this root with the source's in the same statement or
/// transaction, and records nothing once the source has another one. The stamp is the root URL,
/// so a source moved back to the crawled root before the run ends records the run's outcome
/// again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawledRoot {
    pub listing_source_id: ListingSourceId,
    pub crawl_root_url: String,
}

impl CrawledRoot {
    pub fn new(listing_source_id: ListingSourceId, crawl_root_url: impl Into<String>) -> Self {
        Self {
            listing_source_id,
            crawl_root_url: crawl_root_url.into(),
        }
    }
}

/// Whether a write of a spider run's outcome reached the run's ListingSource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlOutcomeWrite {
    /// The source still has the crawled root, and the outcome is recorded.
    Recorded,
    /// The source moved to another crawl root after the run started, or lost its root. Nothing
    /// was written: the run's outcome describes a root the source no longer has.
    RootMoved,
}

impl CrawlOutcomeWrite {
    /// The write of an `UPDATE` guarded by the crawled root, from the rows it changed.
    pub(crate) fn of_guarded_update(rows_affected: u64) -> Self {
        match rows_affected {
            0 => Self::RootMoved,
            _ => Self::Recorded,
        }
    }
}
