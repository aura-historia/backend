//! What `GET /api/reviews` filters and sorts the review queue by. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::{QueryValue, SortDirection};

/// What a pending review waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewWaiting {
    /// A human: the agents stopped on a review of a crawled ListingSource (`human_required`).
    /// Normally none does, since the agents park what they cannot finish.
    Human,
    /// A repair: a needs-repair verdict is open and the agents have not stopped on it.
    Repair,
    /// The agents' retry of its ListingSource, which they parked (`parked`).
    Parked,
    /// A human the crawler handed the product-schema review to because its ListingSource's URL
    /// pattern likely takes non-product pages, in any authoring mode
    /// (`held_for_non_product_loop`).
    NonProductLoop,
}

impl QueryValue for ReviewWaiting {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("human", Self::Human),
        ("repair", Self::Repair),
        ("parked", Self::Parked),
        ("non_product_loop", Self::NonProductLoop),
    ];
}

/// Whether a review's ListingSource is crawled: the header and the Overview count only the
/// pending reviews of crawl-enabled ListingSources, which hold back crawls and scrapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCrawl {
    Enabled,
    Disabled,
}

impl SourceCrawl {
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl QueryValue for SourceCrawl {
    const VALUES: &'static [(&'static str, Self)] =
        &[("enabled", Self::Enabled), ("disabled", Self::Disabled)];
}

/// The order of the review queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewSort {
    /// The order the review agents work: pending reviews of crawl-enabled ListingSources, then
    /// pending reviews of disabled ones, each oldest first, then the closed reviews, newest first.
    #[default]
    Queue,
    Created,
    Updated,
    Source,
}

impl ReviewSort {
    /// The direction the column sorts in when the request names none.
    pub const fn default_direction(self) -> SortDirection {
        match self {
            Self::Source => SortDirection::Ascending,
            _ => SortDirection::Descending,
        }
    }
}

impl QueryValue for ReviewSort {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("queue", Self::Queue),
        ("created", Self::Created),
        ("updated", Self::Updated),
        ("source", Self::Source),
    ];
}
