use super::*;
use crate::shipping::listing::ShippingMatchValuationData;
use domain_primitives::event_id::EventId;
use search_filter_service::ports::{SearchFilterMatchView, SearchFilterView};
use user_core::user_id::UserId;
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilterData {
    #[schemars(schema_with = "code_sets::user_id")]
    user_id: UserId,
    #[schemars(schema_with = "crate::wire::schema::user_search_filter_id")]
    user_search_filter_id: UserSearchFilterId,
    #[schemars(with = "String")]
    name: UserSearchFilterName,
    notifications: bool,
    #[serde(with = "code_sets::state")]
    #[schemars(schema_with = "code_sets::state_schema")]
    state: SearchFilterState,
    search: ProductListingSearchData<ProductListingId, ListingSourceId, AuctionId>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    created: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    updated: Option<OffsetDateTime>,
}

impl From<SearchFilterView> for SearchFilterData {
    fn from(view: SearchFilterView) -> Self {
        Self {
            user_id: view.user_id,
            user_search_filter_id: view.search_filter_id,
            name: view.name,
            notifications: view.notifications,
            state: view.state,
            search: view.search.into(),
            created: Some(view.created),
            updated: Some(view.updated),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilterMatchData {
    #[schemars(schema_with = "code_sets::user_id")]
    user_id: UserId,
    #[schemars(schema_with = "crate::wire::schema::user_search_filter_id")]
    user_search_filter_id: UserSearchFilterId,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    user_search_filter_name: Option<UserSearchFilterName>,
    #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[schemars(schema_with = "crate::wire::schema::event_id")]
    origin_event_id: EventId,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_match_valuation: Option<ShippingMatchValuationData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    feedback: Option<bool>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    updated: OffsetDateTime,
}

impl From<SearchFilterMatchView> for SearchFilterMatchData {
    fn from(view: SearchFilterMatchView) -> Self {
        Self {
            user_id: view.user_id,
            user_search_filter_id: view.search_filter_id,
            user_search_filter_name: view.search_filter_name,
            product_listing_id: view.product_listing_id,
            origin_event_id: view.origin_event_id,
            shipping_match_valuation: view.shipping_match_valuation.map(Into::into),
            feedback: view.feedback,
            created: view.created,
            updated: view.updated,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedData<T> {
    pub items: Vec<T>,
    pub from: u64,
    pub size: u64,
    pub total: Option<u64>,
}

impl From<Vec<SearchFilterView>> for PaginatedData<SearchFilterData> {
    fn from(items: Vec<SearchFilterView>) -> Self {
        let size = items.len() as u64;
        Self {
            items: items.into_iter().map(Into::into).collect(),
            from: 0,
            size,
            total: Some(size),
        }
    }
}
