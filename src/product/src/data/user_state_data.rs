use crate::core::user_state::{ItemUserState, WatchlistUserState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductUserStateData {
    pub watchlist: WatchlistUserStateData,
}
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistUserStateData {
    pub watching: bool,
    pub notifications: bool,
}

impl From<ItemUserState> for ProductUserStateData {
    fn from(value: ItemUserState) -> Self {
        ProductUserStateData {
            watchlist: value.watchlist.into(),
        }
    }
}

impl From<WatchlistUserState> for WatchlistUserStateData {
    fn from(value: WatchlistUserState) -> Self {
        WatchlistUserStateData {
            watching: value.watching,
            notifications: value.notifications,
        }
    }
}
