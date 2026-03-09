#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationReason {
    Watchlist(NotificationWatchlistReason),
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationWatchlistReason {
    StateListed,
    StateAvailable,
    StateReserved,
    StateSold,
    StateRemoved,
    StateUnknown,
    PriceDiscovered,
    PriceDropped,
    PriceIncreased,
    PriceRemoved,
}
