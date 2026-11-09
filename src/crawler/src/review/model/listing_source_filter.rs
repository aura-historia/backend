//! What `GET /api/listing_sources` filters and sorts the ListingSource overview by. The contract
//! is in `docs/crawler/operations.md` (review service).

use super::{QueryValue, SortDirection};

/// Which work of this process a ListingSource is in right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceActivity {
    Crawling,
    Scraping,
    /// Crawling or scraping.
    Active,
}

impl SourceActivity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Crawling => "crawling",
            Self::Scraping => "scraping",
            Self::Active => "active",
        }
    }
}

impl QueryValue for SourceActivity {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("crawling", Self::Crawling),
        ("scraping", Self::Scraping),
        ("active", Self::Active),
    ];
}

/// How a ListingSource stands, which only a crawl-enabled one can be short of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceHealth {
    /// Any of the four conditions below.
    NeedsAttention,
    /// Its crawl waits for an operator.
    NeedsOperator,
    /// URLs wait on a pending product-schema review.
    Blocked,
    /// Its scrapes keep failing.
    ScrapeFailing,
    /// The mass-removal guard holds removals back.
    HeldRemovals,
    /// Crawl-enabled and none of the conditions.
    Healthy,
}

impl SourceHealth {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NeedsAttention => "needs_attention",
            Self::NeedsOperator => "needs_operator",
            Self::Blocked => "blocked",
            Self::ScrapeFailing => "scrape_failing",
            Self::HeldRemovals => "held_removals",
            Self::Healthy => "healthy",
        }
    }
}

impl QueryValue for SourceHealth {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("needs_attention", Self::NeedsAttention),
        ("needs_operator", Self::NeedsOperator),
        ("blocked", Self::Blocked),
        ("scrape_failing", Self::ScrapeFailing),
        ("held_removals", Self::HeldRemovals),
        ("healthy", Self::Healthy),
    ];
}

/// The column the overview sorts by, named as the overview's JSON field. `Attention` ranks the
/// sources an operator has to look at first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceSort {
    #[default]
    Attention,
    ListingSourceName,
    PendingReviews,
    ProductUrls,
    BlockedUrls,
    HeldRemovalUrls,
    ScrapeFailureStreak,
    LlmCallsCount,
    LastSuccessfulCrawl,
    LastSuccessfulScrape,
    NextCrawlAt,
}

impl SourceSort {
    /// The direction the column sorts in when the request names none: names ascending, every
    /// other column with the most, or the latest, first.
    pub const fn default_direction(self) -> SortDirection {
        match self {
            Self::ListingSourceName => SortDirection::Ascending,
            _ => SortDirection::Descending,
        }
    }
}

impl QueryValue for SourceSort {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("attention", Self::Attention),
        ("listing_source_name", Self::ListingSourceName),
        ("pending_reviews", Self::PendingReviews),
        ("product_urls", Self::ProductUrls),
        ("blocked_urls", Self::BlockedUrls),
        ("held_removal_urls", Self::HeldRemovalUrls),
        ("scrape_failure_streak", Self::ScrapeFailureStreak),
        ("llm_calls_count", Self::LlmCallsCount),
        ("last_successful_crawl", Self::LastSuccessfulCrawl),
        ("last_successful_scrape", Self::LastSuccessfulScrape),
        ("next_crawl_at", Self::NextCrawlAt),
    ];
}
