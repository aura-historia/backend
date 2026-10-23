pub const PAGE_ROLE_PRIMARY: &str = "PRIMARY";
pub const PAGE_ROLE_SEED: &str = "SEED";
pub const PAGE_ROLE_TRIGGERING_GENERATION_PAGE: &str = "TRIGGERING_GENERATION_PAGE";
/// A page of the ListingSource a reviewer added to test the candidate beyond its seed pages.
pub const PAGE_ROLE_REVIEWER_ADDED: &str = "REVIEWER_ADDED";
/// A page of the ListingSource that failed every cached schema while the review was pending and
/// waits for its approval.
pub const PAGE_ROLE_WAITING_PAGE: &str = "WAITING_PAGE";
