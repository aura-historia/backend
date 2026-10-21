use crate::patch_value::PatchValue;
use product_listing_core::product_listing_id::ProductListingId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use user_core::user_id::UserId;
use watchlist_core::WatchlistProductListing;
use watchlist_core::watchlist_state::WatchlistState;
use watchlist_service::ports::WatchlistProductListingView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PatchWatchlistStateData {
    Active,
    InactiveByUser,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistEntryData {
    #[schemars(schema_with = "user_id_schema")]
    pub user_id: UserId,
    #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
    pub product_listing_id: ProductListingId,
    pub notifications: bool,
    #[serde(serialize_with = "serialize_watchlist_state")]
    #[schemars(schema_with = "watchlist_state_schema")]
    pub state: WatchlistState,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    pub created: Option<OffsetDateTime>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    pub updated: Option<OffsetDateTime>,
}
impl From<WatchlistProductListing> for WatchlistEntryData {
    fn from(e: WatchlistProductListing) -> Self {
        Self {
            user_id: e.user_id(),
            product_listing_id: e.product_listing_id(),
            notifications: e.notifications(),
            state: e.state(),
            created: None,
            updated: None,
        }
    }
}
impl From<WatchlistProductListingView> for WatchlistEntryData {
    fn from(v: WatchlistProductListingView) -> Self {
        Self {
            user_id: v.user_id,
            product_listing_id: v.product_listing_id,
            notifications: v.notifications,
            state: v.state,
            created: Some(v.created),
            updated: Some(v.updated),
        }
    }
}

pub fn watchlist_state(state: PatchWatchlistStateData) -> WatchlistState {
    match state {
        PatchWatchlistStateData::Active => WatchlistState::Active,
        PatchWatchlistStateData::InactiveByUser => WatchlistState::InactiveByUser,
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PostWatchlistData {
    pub product_listing_id: String,
    pub notifications: Option<bool>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PatchWatchlistData {
    #[serde(default)]
    pub notifications: PatchValue<bool>,
    #[serde(default)]
    pub state: PatchValue<PatchWatchlistStateData>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListWatchlistQueryData {
    #[serde(default, with = "crate::wire::language")]
    #[schemars(schema_with = "crate::wire::language::schema")]
    pub language: localization::Language,
    #[serde(default, with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    pub currency: money::Currency,
    pub size: Option<u64>,
    /// JSON encoded [RFC3339 watchlist creation timestamp, ProductListing ID].
    pub search_after: Option<String>,
}

fn serialize_watchlist_state<S: serde::Serializer>(
    value: &WatchlistState,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(value.as_str())
}

fn user_id_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    crate::wire::schema::object_id(UserId::PREFIX)
}

fn watchlist_state_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    use strum::IntoEnumIterator;
    schemars::json_schema!({"type": "string", "enum": WatchlistState::iter().map(WatchlistState::as_str).collect::<Vec<_>>()})
}
