use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NotificationReasonRecord {
    WatchlistStateListed,
    WatchlistStateAvailable,
    WatchlistStateReserved,
    WatchlistStateSold,
    WatchlistStateRemoved,
    WatchlistStateUnknown,
    WatchlistPriceDiscovered,
    WatchlistPriceDropped,
    WatchlistPriceIncreased,
    WatchlistPriceRemoved,
}

impl NotificationReasonRecord {
    pub fn is_watchlist(&self) -> bool {
        matches!(
            self,
            Self::WatchlistStateListed
                | Self::WatchlistStateAvailable
                | Self::WatchlistStateReserved
                | Self::WatchlistStateSold
                | Self::WatchlistStateRemoved
                | Self::WatchlistStateUnknown
                | Self::WatchlistPriceDiscovered
                | Self::WatchlistPriceDropped
                | Self::WatchlistPriceIncreased
                | Self::WatchlistPriceRemoved
        )
    }
}
