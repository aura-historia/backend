use common::resource_state::data::{PatchResourceStateData, ResourceStateData};
use common::user_id::UserId;
use product_core::product_id::ProductId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use watchlist_core::WatchlistProduct;
use watchlist_service::ports::WatchlistProductView;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WatchlistEntryData {
    pub(crate) user_id: UserId,
    pub(crate) product_id: ProductId,
    pub(crate) notifications: bool,
    pub(crate) state: ResourceStateData,
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
            state: e.state().into(),
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
            state: v.state.into(),
            created: Some(v.created),
            updated: Some(v.updated),
        }
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
    pub(crate) notifications: Option<bool>,
    pub(crate) state: Option<PatchResourceStateData>,
}
