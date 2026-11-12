use super::{ProductListingSearchData, code_sets};
use crate::listing_sources::PublicListingSourceData;
use crate::product_listings::product_data::{
    PersonalizedProductListingSummaryData, personalized_product_summary_data,
};
use crate::shipping::listing::ShippingMatchValuationData;
use auction_core::AuctionId;
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::ProductListingId;
use schemars::JsonSchema;
use search_filter_core::{
    search_filter_state::SearchFilterState, user_search_filter_id::UserSearchFilterId,
    user_search_filter_name::UserSearchFilterName,
};
use search_filter_service::ports::SearchFilterMatchView;
use search_filter_service::use_cases::{SearchFilterDetails, SearchFilterMatchPreview};
use serde::Serialize;
use time::OffsetDateTime;
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
    /// Public cards of the ListingSources `search` includes or excludes, one per ID that still
    /// resolves; IDs of removed sources stay in `search` but have no card. Read-only.
    listing_sources: Vec<PublicListingSourceData>,
}

impl From<SearchFilterDetails> for SearchFilterData {
    fn from(details: SearchFilterDetails) -> Self {
        let view = details.view;
        Self {
            user_id: view.user_id,
            user_search_filter_id: view.search_filter_id,
            name: view.name,
            notifications: view.notifications,
            state: view.state,
            search: view.search.into(),
            created: Some(view.created),
            updated: Some(view.updated),
            listing_sources: details
                .listing_sources
                .into_iter()
                .map(Into::into)
                .collect(),
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
pub struct SearchFilterMatchPreviewData {
    /// At most four listings the criteria match now, most relevant first.
    pub items: Vec<PersonalizedProductListingSummaryData>,
    /// `true` when fewer than four matches were found among the candidates the preview
    /// evaluates, so more listings may match than it shows.
    pub candidate_limit_reached: bool,
}

impl From<SearchFilterMatchPreview> for SearchFilterMatchPreviewData {
    fn from(preview: SearchFilterMatchPreview) -> Self {
        Self {
            items: preview
                .items
                .into_iter()
                .map(personalized_product_summary_data)
                .collect(),
            candidate_limit_reached: preview.candidate_limit_reached,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedData<T> {
    pub items: Vec<T>,
    #[schemars(range(max = u64::MAX))]
    pub from: u64,
    #[schemars(range(max = u64::MAX))]
    pub size: u64,
    #[schemars(range(max = u64::MAX))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl From<Vec<SearchFilterDetails>> for PaginatedData<SearchFilterData> {
    fn from(items: Vec<SearchFilterDetails>) -> Self {
        let size = items.len() as u64;
        Self {
            items: items.into_iter().map(Into::into).collect(),
            from: 0,
            size,
            total: Some(size),
        }
    }
}
