use product_core::product_id::ProductId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use user_core::user_id::UserId;
use watchlist_core::{ResourceState, WatchlistProduct};
use watchlist_service::ports::WatchlistProductView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ResourceStateData {
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum PatchResourceStateData {
    Active,
    InactiveByUser,
}

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
            state: resource_state_data(e.state()),
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
            state: resource_state_data(v.state),
            created: Some(v.created),
            updated: Some(v.updated),
        }
    }
}

pub(crate) fn resource_state_data(state: ResourceState) -> ResourceStateData {
    match state {
        ResourceState::Active => ResourceStateData::Active,
        ResourceState::InactiveByUser => ResourceStateData::InactiveByUser,
        ResourceState::InactiveByRestrictedPlan => ResourceStateData::InactiveByRestrictedPlan,
    }
}

pub(crate) fn watchlist_state(state: PatchResourceStateData) -> ResourceState {
    match state {
        PatchResourceStateData::Active => ResourceState::Active,
        PatchResourceStateData::InactiveByUser => ResourceState::InactiveByUser,
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
