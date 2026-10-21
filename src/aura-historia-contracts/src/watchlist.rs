use crate::patch_value::PatchValue;
use application::pagination::Cursor;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::ProductListingWatchlistDetailsCursor;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use user_core::user_id::UserId;
use watchlist_core::WatchlistProductListing;
use watchlist_core::watchlist_state::WatchlistState;
use watchlist_service::{
    ports::WatchlistProductListingView,
    use_cases::{
        ListWatchlistRequest, ListWatchlistResult, UpdateWatchlistProductListingCommand,
        WatchProductListingCommand,
    },
};

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

fn watchlist_state(state: PatchWatchlistStateData) -> WatchlistState {
    match state {
        PatchWatchlistStateData::Active => WatchlistState::Active,
        PatchWatchlistStateData::InactiveByUser => WatchlistState::InactiveByUser,
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PostWatchlistData {
    #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
    pub product_listing_id: String,
    pub notifications: Option<bool>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PatchWatchlistData {
    #[serde(default)]
    #[schemars(schema_with = "bool::json_schema")]
    pub notifications: PatchValue<bool>,
    #[serde(default)]
    #[schemars(schema_with = "PatchWatchlistStateData::json_schema")]
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

impl ListWatchlistQueryData {
    pub fn into_request(
        self,
        user_id: UserId,
    ) -> Result<ListWatchlistRequest, WatchlistCursorError> {
        Ok(ListWatchlistRequest {
            user_id,
            language: self.language,
            currency: self.currency,
            cursor: watchlist_cursor(self.size, self.search_after.as_deref())?,
        })
    }
}

/// REST-compatible pagination reports the returned item count and a JSON cursor array.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CursoredWatchlistData {
    pub items: Vec<crate::product_listings::product_data::PersonalizedProductListingDetailsData>,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<(String, String)>")]
    pub search_after: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl TryFrom<ListWatchlistResult> for CursoredWatchlistData {
    type Error = time::error::Format;

    fn try_from(result: ListWatchlistResult) -> Result<Self, Self::Error> {
        Ok(Self {
            size: result.items.len() as u64,
            items: result
                .items
                .into_iter()
                .map(crate::product_listings::product_data::personalized_product_details_data)
                .collect(),
            search_after: result
                .cursor
                .search_after
                .map(watchlist_cursor_value)
                .transpose()?,
            total: result.total,
        })
    }
}

pub fn watchlist_cursor(
    size: Option<u64>,
    search_after: Option<&str>,
) -> Result<Cursor<ProductListingWatchlistDetailsCursor>, WatchlistCursorError> {
    Ok(Cursor {
        size: size.unwrap_or(21).clamp(1, 100),
        search_after: search_after
            .map(|value| {
                serde_json::from_str(value)
                    .map_err(WatchlistCursorError::Json)
                    .and_then(parse_watchlist_cursor)
            })
            .transpose()?,
    })
}

pub fn parse_watchlist_cursor(
    value: serde_json::Value,
) -> Result<ProductListingWatchlistDetailsCursor, WatchlistCursorError> {
    use serde_json::Value;
    let Value::Array(values) = value else {
        return Err(WatchlistCursorError::Array);
    };
    let [Value::String(created), Value::String(product_listing_id)] = values.as_slice() else {
        return Err(WatchlistCursorError::Values);
    };
    Ok(ProductListingWatchlistDetailsCursor {
        watchlist_created: OffsetDateTime::parse(
            created,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(WatchlistCursorError::Timestamp)?,
        product_listing_id: product_listing_id
            .parse()
            .map_err(WatchlistCursorError::ProductListingId)?,
    })
}

pub fn watchlist_cursor_value(
    cursor: ProductListingWatchlistDetailsCursor,
) -> Result<serde_json::Value, time::error::Format> {
    cursor
        .watchlist_created
        .format(&time::format_description::well_known::Rfc3339)
        .map(|created| serde_json::json!([created, cursor.product_listing_id]))
}

#[derive(Debug, thiserror::Error)]
pub enum WatchlistCursorError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("searchAfter must be a JSON array containing timestamp and product ID.")]
    Array,
    #[error("searchAfter must contain an RFC3339 timestamp and ProductListing ID.")]
    Values,
    #[error(transparent)]
    Timestamp(#[from] time::error::Parse),
    #[error("must be a valid ProductListing ID")]
    ProductListingId(#[source] domain_primitives::object_id::ObjectIdError),
}

impl PatchWatchlistData {
    pub fn into_command(
        self,
        user_id: UserId,
        product_listing_id: ProductListingId,
        presentation: crate::product_listings::presentation::ProductListingPresentationData,
    ) -> Result<UpdateWatchlistProductListingCommand, WatchlistPatchError> {
        Ok(UpdateWatchlistProductListingCommand {
            user_id,
            product_listing_id,
            notifications: non_nullable(self.notifications, "notifications")?,
            state: non_nullable(self.state, "state")?.map(watchlist_state),
            language: presentation.language,
            currency: presentation.currency,
        })
    }
}

fn non_nullable<T>(
    value: PatchValue<T>,
    field: &'static str,
) -> Result<Option<T>, WatchlistPatchError> {
    match value {
        PatchValue::Omitted => Ok(None),
        PatchValue::Value(value) => Ok(Some(value)),
        PatchValue::Null => Err(WatchlistPatchError { field }),
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Body field '{field}' must not be null.")]
pub struct WatchlistPatchError {
    pub field: &'static str,
}

impl PostWatchlistData {
    pub fn into_command(
        self,
        user_id: UserId,
    ) -> Result<WatchProductListingCommand, domain_primitives::object_id::ObjectIdError> {
        Ok(WatchProductListingCommand {
            user_id,
            product_listing_id: self.product_listing_id.parse()?,
            notifications: self.notifications.unwrap_or(true),
        })
    }
}
