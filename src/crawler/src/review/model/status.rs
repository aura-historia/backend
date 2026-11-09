pub const STATUS_PENDING_REVIEW: &str = "PENDING_REVIEW";
pub const STATUS_APPROVED: &str = "APPROVED";
pub const STATUS_REJECTED: &str = "REJECTED";
pub const STATUS_SUPERSEDED: &str = "SUPERSEDED";

/// A review's status as a console list names it in its `status` filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewStatus {
    PendingReview,
    Approved,
    Rejected,
    Superseded,
}

impl ReviewStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PendingReview => STATUS_PENDING_REVIEW,
            Self::Approved => STATUS_APPROVED,
            Self::Rejected => STATUS_REJECTED,
            Self::Superseded => STATUS_SUPERSEDED,
        }
    }
}

impl super::QueryValue for ReviewStatus {
    const VALUES: &'static [(&'static str, Self)] = &[
        (STATUS_PENDING_REVIEW, Self::PendingReview),
        (STATUS_APPROVED, Self::Approved),
        (STATUS_REJECTED, Self::Rejected),
        (STATUS_SUPERSEDED, Self::Superseded),
    ];
}
