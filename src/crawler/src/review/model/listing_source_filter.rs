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
    /// Any of the six conditions below.
    NeedsAttention,
    /// Crawl-enabled and none of the conditions.
    Healthy,
    /// Crawling is switched off, and no human labelled the source.
    Disabled,
    /// A human labelled the source: its crawling is off until the label is cleared.
    Labelled,
    /// The scope admits the source, which waits for its qualification.
    Unqualified,
    /// Qualified as a real art and antiques e-shop.
    Qualified,
    /// Disqualified: not an art and antiques e-shop the crawler may reach.
    Disqualified,
    /// Its qualification could not be decided: a human's, or the agents' until they try again.
    QualificationNeedsHuman,
    /// The review agents parked it, with a label that expires or an undecided qualification, until
    /// they try it again.
    Parked,
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
    /// Product URLs are in an identity conflict: their schemas derive another identity than
    /// the one their listings were captured under, so they are not captured.
    IdentityConflict,
    /// Crawl-enabled, and neither its scraper nor its spider has worked it for
    /// [`STALE_AFTER_DAYS`]: its `idle_since`, or its creation when it was never worked, is older.
    /// Informational, not one of the conditions that need attention.
    Stale,
    /// Crawl-enabled, and its shop's rate limit holds its next crawl back right now.
    RateLimited,
    /// Crawl-enabled, and its scraper circuit pauses its scrapes right now.
    ScrapePaused,
    /// Crawl-enabled, and its latest spider run was cut short by its time budget or bounded by
    /// its page budget, so it did not crawl the shop completely.
    Truncated,
    /// The scope admits it to crawling with a crawl root; a label or a missing qualification may
    /// still keep it from being crawled.
    InScope,
    /// Crawling is switched on.
    CrawlEnabled,
    /// Crawl-enabled, with live product schemas the scraper captures its pages with.
    WithSchema,
}

/// How many days a crawl-enabled ListingSource may go without a scraped page or a spider run
/// before it counts as stale: three missed daily crawl and scrape cycles.
pub const STALE_AFTER_DAYS: i64 = 3;

impl QueryValue for SourceStatus {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("needs_attention", Self::NeedsAttention),
        ("healthy", Self::Healthy),
        ("disabled", Self::Disabled),
        ("labelled", Self::Labelled),
        ("unqualified", Self::Unqualified),
        ("qualified", Self::Qualified),
        ("disqualified", Self::Disqualified),
        ("qualification_needs_human", Self::QualificationNeedsHuman),
        ("parked", Self::Parked),
        ("crawl_failing", Self::CrawlFailing),
        ("crawl_needs_operator", Self::CrawlNeedsOperator),
        ("blocked", Self::Blocked),
        ("failing", Self::Failing),
        ("held", Self::Held),
        ("identity_conflict", Self::IdentityConflict),
        ("stale", Self::Stale),
        ("rate_limited", Self::RateLimited),
        ("scrape_paused", Self::ScrapePaused),
        ("truncated", Self::Truncated),
        ("in_scope", Self::InScope),
        ("crawl_enabled", Self::CrawlEnabled),
        ("with_schema", Self::WithSchema),
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
    /// When the source was last worked (`idle_since`), its creation when it never was.
    IdleSince,
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
        ("idle_since", Self::IdleSince),
    ];
}
