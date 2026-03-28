use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NotificationReasonRecord {
    WatchlistStateChanged,
    WatchlistPriceChanged,
    SearchFilterMatch,
}

impl NotificationReasonRecord {
    pub fn is_watchlist(&self) -> bool {
        matches!(
            self,
            Self::WatchlistStateChanged | Self::WatchlistPriceChanged
        )
    }

    pub fn is_search_filter(&self) -> bool {
        matches!(self, Self::SearchFilterMatch)
    }
}
