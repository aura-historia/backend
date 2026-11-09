//! What `GET /api/reviews` filters and sorts the review queue by. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::{QueryValue, SortDirection};

/// What a pending review waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewWaiting {
    /// A human: the agents stopped on it (`human_required`).
    Human,
    /// A repair: a needs-repair verdict is open and the agents have not stopped on it.
    Repair,
}

impl QueryValue for ReviewWaiting {
    const VALUES: &'static [(&'static str, Self)] =
        &[("human", Self::Human), ("repair", Self::Repair)];
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
