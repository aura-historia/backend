pub const STATUS_PENDING_REVIEW: &str = "PENDING_REVIEW";
pub const STATUS_APPROVED: &str = "APPROVED";
pub const STATUS_REJECTED: &str = "REJECTED";
/// Legacy terminal status of reviews closed by a needs-repair verdict before such a verdict kept
/// the review pending with a repair request; no verdict writes it any more.
pub const STATUS_NEEDS_REPAIR: &str = "NEEDS_REPAIR";
pub const STATUS_SUPERSEDED: &str = "SUPERSEDED";
