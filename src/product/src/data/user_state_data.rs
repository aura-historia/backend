use crate::core::user_state::{ProductUserState, ProhibitedContentUserState, WatchlistUserState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductUserStateData {
    pub watchlist: WatchlistUserStateData,
    pub prohibited_content: ProhibitedContentUserStateData,
}
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistUserStateData {
    pub watching: bool,
    pub notifications: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProhibitedContentUserStateData {
    pub consent: bool,
}

impl From<ProductUserState> for ProductUserStateData {
    fn from(value: ProductUserState) -> Self {
        ProductUserStateData {
            watchlist: value.watchlist.into(),
            prohibited_content: value.prohibited_content.into(),
        }
    }
}

impl From<ProhibitedContentUserState> for ProhibitedContentUserStateData {
    fn from(value: ProhibitedContentUserState) -> Self {
        ProhibitedContentUserStateData {
            consent: value.consent,
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
