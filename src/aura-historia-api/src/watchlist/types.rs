use crate::patch_value::PatchValue;
use product_core::product_id::ProductId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use user_core::user_id::UserId;
use watchlist_core::WatchlistProduct;
use watchlist_core::watchlist_state::WatchlistState;
use watchlist_service::ports::WatchlistProductView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum PatchWatchlistStateData {
    Active,
    InactiveByUser,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WatchlistEntryData {
    pub(crate) user_id: UserId,
    pub(crate) product_id: ProductId,
    pub(crate) notifications: bool,
    #[serde(with = "crate::wire::watchlist_state")]
    pub(crate) state: WatchlistState,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub(crate) created: Option<OffsetDateTime>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub(crate) updated: Option<OffsetDateTime>,
}
impl From<WatchlistProduct> for WatchlistEntryData {
    fn from(e: WatchlistProduct) -> Self {
        Self {
            user_id: e.user_id(),
            product_id: e.product_id(),
            notifications: e.notifications(),
            state: e.state(),
            created: None,
            updated: None,
        }
    }
}
impl From<WatchlistProductView> for WatchlistEntryData {
    fn from(v: WatchlistProductView) -> Self {
        Self {
            user_id: v.user_id,
            product_id: v.product_id,
            notifications: v.notifications,
            state: v.state,
            created: Some(v.created),
            updated: Some(v.updated),
        }
    }
}

pub(crate) fn watchlist_state(state: PatchWatchlistStateData) -> WatchlistState {
    match state {
        PatchWatchlistStateData::Active => WatchlistState::Active,
        PatchWatchlistStateData::InactiveByUser => WatchlistState::InactiveByUser,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PostWatchlistData {
    pub(crate) product_id: ProductId,
    pub(crate) notifications: Option<bool>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchWatchlistData {
    #[serde(default)]
    pub(crate) notifications: PatchValue<bool>,
    #[serde(default)]
    pub(crate) state: PatchValue<PatchWatchlistStateData>,
}
