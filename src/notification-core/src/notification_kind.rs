#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationKind {
    WatchlistPriceChanged,
    WatchlistStateChanged,
    SearchFilterMatch,
    PartnerApplicationApproved,
    PartnerApplicationRejected,
}
