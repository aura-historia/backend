#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartnerShopApplicationState {
    Submitted,
    InReview,
    Rejected,
    Approved,
}
