//! What `GET /api/listing_sources` filters and sorts the ListingSource overview by. The contract
//! is in `docs/crawler/operations.md` (review service).

use super::{QueryValue, SortDirection};

/// Which work of this process a ListingSource is in right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceActivity {
    Crawling,
    Scraping,
    /// Neither crawling nor scraping.
    Idle,
}

impl QueryValue for SourceActivity {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("crawling", Self::Crawling),
        ("scraping", Self::Scraping),
        ("idle", Self::Idle),
    ];
}

/// How a ListingSource stands. Only a crawl-enabled one can need attention; `Disabled` and
/// `Labelled` are disjoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceStatus {
    /// Any of the five conditions below.
    NeedsAttention,
    /// Crawl-enabled and none of the conditions.
    Healthy,
    /// Crawling is switched off, and no human labelled the source.
    Disabled,
    /// A human labelled the source: its crawling is off until the label is cleared.
    Labelled,
    /// Its last crawl failed and is retried by the schedule.
    CrawlFailing,
    /// Its crawl waits for an operator.
    CrawlNeedsOperator,
    /// URLs wait on a pending product-schema review.
    Blocked,
    /// Its scrapes keep failing.
    Failing,
    /// The mass-removal guard holds removals back.
    Held,
}

impl QueryValue for SourceStatus {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("needs_attention", Self::NeedsAttention),
        ("healthy", Self::Healthy),
        ("disabled", Self::Disabled),
        ("labelled", Self::Labelled),
        ("crawl_failing", Self::CrawlFailing),
        ("crawl_needs_operator", Self::CrawlNeedsOperator),
        ("blocked", Self::Blocked),
        ("failing", Self::Failing),
        ("held", Self::Held),
    ];
}

/// The column the overview sorts by. `Attention` ranks the sources an operator has to look at
/// first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceSort {
    #[default]
    Attention,
    Name,
    PendingReviews,
    LastSuccessfulScrape,
    NextCrawlAt,
}

impl SourceSort {
    /// The direction the column sorts in when the request names none: names ascending, every
    /// other column with the most, or the latest, first.
    pub const fn default_direction(self) -> SortDirection {
        match self {
            Self::Name => SortDirection::Ascending,
            _ => SortDirection::Descending,
        }
    }
}

impl QueryValue for SourceSort {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("attention", Self::Attention),
        ("name", Self::Name),
        ("pending_reviews", Self::PendingReviews),
        ("last_successful_scrape", Self::LastSuccessfulScrape),
        ("next_crawl_at", Self::NextCrawlAt),
    ];
}
