use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::shipping::listing::ShippingMatchValuationData;

use application::patch_field::PatchField;
use auction_core::AuctionId;
use domain_primitives::event_id::EventId;

use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use localization::Language;
use money::Currency;

use listing_source_core::ListingSourceId;
use money::MonetaryAmount;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_orderability::ListingOrderability;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use user_core::user_id::UserId;

use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, ListingAvailabilityQuery, ProductListingSearch,
};
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_service::ports::{SearchFilterMatchView, SearchFilterView};
use search_filter_service::use_cases::ProductListingSearchPatch;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use std::collections::HashSet;
use std::hash::Hash;
use std::str::FromStr;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum PatchSearchFilterStateData {
    Active,
    InactiveByUser,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateSearchFilterData {
    #[serde(default)]
    pub(super) accept_shipping_destination:
        Option<crate::shipping_search::AcceptShippingDestinationData>,
    pub(super) name: UserSearchFilterName,
    #[serde(default = "default_notifications")]
    pub(super) notifications: bool,
    pub(super) search: ProductListingSearchData,
}

fn default_notifications() -> bool {
    true
}

fn serialize_optional_set_code<T, S>(
    values: &Option<HashSet<T>>,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy + Eq + std::hash::Hash,
    S: Serializer,
{
    match values {
        Some(values) => serializer.collect_seq(values.iter().map(|value| code(*value))),
        None => serializer.serialize_none(),
    }
}

fn deserialize_optional_set_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<Option<HashSet<T>>, D::Error>
where
    T: Eq + std::hash::Hash,
    D: Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer)?.map_or(Ok(None), |values| {
        values
            .into_iter()
            .map(|value| {
                parse(&value)
                    .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
            })
            .collect::<Result<HashSet<_>, D::Error>>()
            .map(Some)
    })
}

fn deserialize_patch_set_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<PatchValue<HashSet<T>>, D::Error>
where
    T: Eq + std::hash::Hash,
    D: Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |values| {
        values
            .into_iter()
            .map(|value| {
                parse(&value)
                    .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
            })
            .collect::<Result<HashSet<_>, D::Error>>()
            .map(PatchValue::Value)
    })
}

mod listing_availability_set_option {
    use super::*;

    pub(super) fn serialize<S>(
        values: &Option<HashSet<ListingAvailability>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_optional_set_code(values, serializer, ListingAvailability::as_str)
    }

    pub(super) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<HashSet<ListingAvailability>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_optional_set_code(deserializer, ListingAvailability::from_code)
    }
}

mod listing_orderability_set_option {
    use super::*;

    pub(super) fn serialize<S>(
        values: &Option<HashSet<ListingOrderability>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_optional_set_code(values, serializer, ListingOrderability::as_str)
    }

    pub(super) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<HashSet<ListingOrderability>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_optional_set_code(deserializer, ListingOrderability::from_code)
    }
}

mod listing_orderability_patch_set {
    use super::*;

    pub(super) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<PatchValue<HashSet<ListingOrderability>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_patch_set_code(deserializer, ListingOrderability::from_code)
    }
}

type UpdateSearchFilterFields = (
    PatchField<UserSearchFilterName>,
    PatchField<bool>,
    PatchField<SearchFilterState>,
    ProductListingSearchPatch,
);

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateSearchFilterData {
    #[serde(default)]
    pub(super) accept_shipping_destination:
        Option<crate::shipping_search::AcceptShippingDestinationData>,
    #[serde(default)]
    pub(super) name: Option<UserSearchFilterName>,
    #[serde(default)]
    pub(super) notifications: Option<bool>,
    #[serde(default)]
    pub(super) state: Option<PatchSearchFilterStateData>,
    #[serde(default)]
    pub(super) search: Option<ProductListingSearchPatchData>,
}

impl UpdateSearchFilterData {
    pub(super) fn into_fields(self) -> Result<UpdateSearchFilterFields, crate::error::ApiError> {
        let search = match self.search {
            Some(search) => search.try_into_patch()?,
            None => ProductListingSearchPatch::default(),
        };

        Ok((
            self.name.map_or(PatchField::Unchanged, PatchField::Set),
            self.notifications
                .map_or(PatchField::Unchanged, PatchField::Set),
            self.state
                .map(search_filter_state)
                .map_or(PatchField::Unchanged, PatchField::Set),
            search,
        ))
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProductListingSearchPatchData {
    #[serde(default)]
    geography: PatchValue<crate::geographic_search::GeographicSearchData>,
    #[serde(default)]
    shipping: PatchValue<crate::shipping_search::ShippingSearchData>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::language::patch::deserialize")]
    language: PatchValue<Language>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::currency::patch::deserialize")]
    currency: PatchValue<Currency>,
    #[serde(rename = "productQuery", default)]
    product_listing_query: PatchValue<Vec<TextQuery<1>>>,
    #[serde(rename = "enhancedSearchDescription", default)]
    enhanced_search_description: PatchValue<String>,
    #[serde(rename = "listingSourceId", default)]
    listing_source_id_query: PatchValue<HashSet<String>>,
    #[serde(rename = "excludeListingSourceId", default)]
    exclude_listing_source_id_query: PatchValue<HashSet<String>>,
    #[serde(rename = "auctionId", default)]
    auction_id_query: PatchValue<HashSet<String>>,
    #[serde(rename = "price", default)]
    price_query: PatchValue<RangeQuery<u64>>,
    #[serde(rename = "availability", default)]
    #[serde(deserialize_with = "crate::wire::listing_availability::patch_set::deserialize")]
    availability_query: PatchValue<HashSet<ListingAvailability>>,
    #[serde(rename = "orderability", default)]
    #[serde(deserialize_with = "listing_orderability_patch_set::deserialize")]
    orderability_query: PatchValue<HashSet<ListingOrderability>>,
    #[serde(rename = "includeUnspecifiedAvailability", default)]
    include_unspecified_availability: PatchValue<bool>,
    #[serde(
        rename = "created",
        default,
        deserialize_with = "crate::patch_value::rfc3339_range::deserialize"
    )]
    created_query: PatchValue<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "updated",
        default,
        deserialize_with = "crate::patch_value::rfc3339_range::deserialize"
    )]
    updated_query: PatchValue<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "lotBiddingOpens",
        default,
        deserialize_with = "crate::patch_value::rfc3339_range::deserialize"
    )]
    lot_bidding_opens_query: PatchValue<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "lotScheduledCloses",
        default,
        deserialize_with = "crate::patch_value::rfc3339_range::deserialize"
    )]
    lot_scheduled_closes_query: PatchValue<RangeQuery<OffsetDateTime>>,
}

impl ProductListingSearchPatchData {
    fn try_into_patch(self) -> Result<ProductListingSearchPatch, crate::error::ApiError> {
        Ok(ProductListingSearchPatch {
            geography: geographic_patch(self.geography, "search.geography")?,
            shipping: match self.shipping {
                PatchValue::Omitted => PatchField::Unchanged,
                PatchValue::Null => PatchField::Clear,
                PatchValue::Value(d) => match crate::shipping_search::parse(Some(d), true)? {
                    Some(q) => PatchField::Set(q),
                    None => PatchField::Clear,
                },
            },
            language: non_nullable_patch(self.language, "search.language")?,
            currency: non_nullable_patch(self.currency, "search.currency")?,
            product_listing_query: non_nullable_patch(
                self.product_listing_query,
                "search.productQuery",
            )?,
            enhanced_search_description: match self.enhanced_search_description {
                PatchValue::Omitted => PatchField::Unchanged,
                PatchValue::Null => PatchField::Clear,
                PatchValue::Value(value) => PatchField::Set(
                    EnhancedSearchDescription::try_from(value).map_err(|error| {
                        crate::error::ApiError::bad_request(crate::error::BAD_BODY_VALUE)
                            .with_detail(error.to_string())
                    })?,
                ),
            },
            listing_source_id_query: non_nullable_patch(
                parse_body_object_id_patch::<ListingSourceId>(
                    self.listing_source_id_query,
                    "search.listingSourceId",
                    "ListingSource",
                )?
                .map(Into::into),
                "search.listingSourceId",
            )?,
            exclude_listing_source_id_query: non_nullable_patch(
                parse_body_object_id_patch::<ListingSourceId>(
                    self.exclude_listing_source_id_query,
                    "search.excludeListingSourceId",
                    "ListingSource",
                )?
                .map(Into::into),
                "search.excludeListingSourceId",
            )?,
            auction_id_query: clearable(
                parse_body_object_id_patch::<AuctionId>(
                    self.auction_id_query,
                    "search.auctionId",
                    "Auction",
                )?
                .map(Into::into),
            ),
            price_query: clearable(
                self.price_query
                    .map(|query| query.map(MonetaryAmount::from)),
            ),
            availability_query: availability_query_patch(
                self.availability_query,
                self.orderability_query,
                self.include_unspecified_availability,
            )?,
            created_query: clearable(self.created_query),
            updated_query: clearable(self.updated_query),
            lot_bidding_opens_query: clearable(self.lot_bidding_opens_query),
            lot_scheduled_closes_query: clearable(self.lot_scheduled_closes_query),
        })
    }
}

fn parse_body_object_ids<T>(
    values: HashSet<String>,
    field: &'static str,
    semantic_type: &'static str,
) -> Result<HashSet<T>, crate::error::ApiError>
where
    T: FromStr + Eq + Hash,
{
    values
        .into_iter()
        .map(|value| crate::wire::parse_body_object_id(&value, field, semantic_type))
        .collect()
}

fn parse_body_object_id_patch<T>(
    values: PatchValue<HashSet<String>>,
    field: &'static str,
    semantic_type: &'static str,
) -> Result<PatchValue<HashSet<T>>, crate::error::ApiError>
where
    T: FromStr + Eq + Hash,
{
    match values {
        PatchValue::Omitted => Ok(PatchValue::Omitted),
        PatchValue::Null => Ok(PatchValue::Null),
        PatchValue::Value(values) => {
            parse_body_object_ids(values, field, semantic_type).map(PatchValue::Value)
        }
    }
}

fn availability_query_from_parts(
    availability: Option<HashSet<ListingAvailability>>,
    orderability: Option<HashSet<ListingOrderability>>,
    include_unspecified: Option<bool>,
) -> Option<ListingAvailabilityQuery> {
    if availability.is_none() && orderability.is_none() && include_unspecified.is_none() {
        None
    } else {
        Some(ListingAvailabilityQuery {
            any_of: availability.unwrap_or_default().into(),
            orderability: orderability.unwrap_or_default().into(),
            include_unspecified: include_unspecified.unwrap_or(false),
        })
    }
}

fn availability_query_patch(
    availability: PatchValue<HashSet<ListingAvailability>>,
    orderability: PatchValue<HashSet<ListingOrderability>>,
    include_unspecified: PatchValue<bool>,
) -> Result<PatchField<ListingAvailabilityQuery>, crate::error::ApiError> {
    match (availability, orderability, include_unspecified) {
        (PatchValue::Omitted, PatchValue::Omitted, PatchValue::Omitted) => {
            Ok(PatchField::Unchanged)
        }
        (PatchValue::Null, PatchValue::Null, PatchValue::Null) => Ok(PatchField::Clear),
        (
            PatchValue::Value(availability),
            PatchValue::Value(orderability),
            PatchValue::Value(include_unspecified),
        ) => Ok(PatchField::Set(ListingAvailabilityQuery {
            any_of: availability.into(),
            orderability: orderability.into(),
            include_unspecified,
        })),
        _ => Err(
            crate::error::ApiError::bad_request(crate::error::BAD_BODY_VALUE).with_detail(
                "Availability query fields must be supplied together, or all null to clear the query.",
            ),
        ),
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateSearchFilterMatchFeedbackData {
    #[serde(default)]
    pub(super) feedback: PatchValue<bool>,
}

impl UpdateSearchFilterMatchFeedbackData {
    pub(super) fn feedback(self) -> PatchField<bool> {
        clearable(self.feedback)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(
    deserialize = "ProductId: Deserialize<'de> + Eq + Hash, SourceId: Deserialize<'de> + Eq + Hash, AuctionIdValue: Deserialize<'de> + Eq + Hash"
))]
pub(super) struct ProductListingSearchData<
    ProductId = String,
    SourceId = String,
    AuctionIdValue = String,
> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    geography: Option<crate::geographic_search::GeographicSearchData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shipping: Option<crate::shipping_search::ShippingSearchData>,
    #[serde(default, with = "crate::wire::language")]
    language: Language,
    #[serde(default, with = "crate::wire::currency")]
    currency: Currency,
    #[serde(
        rename = "productQuery",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    product_listing_query: Vec<TextQuery<1>>,
    #[serde(
        rename = "enhancedSearchDescription",
        skip_serializing_if = "Option::is_none",
        default
    )]
    enhanced_search_description: Option<String>,
    #[serde(
        rename = "excludeProductListingId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    exclude_product_listing_id_query: HashSet<ProductId>,
    #[serde(
        rename = "listingSourceId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    listing_source_id_query: HashSet<SourceId>,
    #[serde(
        rename = "excludeListingSourceId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    exclude_listing_source_id_query: HashSet<SourceId>,
    #[serde(
        rename = "auctionId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    auction_id_query: HashSet<AuctionIdValue>,
    #[serde(rename = "price", skip_serializing_if = "Option::is_none", default)]
    price_query: Option<RangeQuery<u64>>,
    #[serde(
        rename = "availability",
        skip_serializing_if = "Option::is_none",
        default,
        with = "listing_availability_set_option"
    )]
    availability_query: Option<HashSet<ListingAvailability>>,
    #[serde(
        rename = "orderability",
        skip_serializing_if = "Option::is_none",
        default,
        with = "listing_orderability_set_option"
    )]
    orderability_query: Option<HashSet<ListingOrderability>>,
    #[serde(
        rename = "includeUnspecifiedAvailability",
        skip_serializing_if = "Option::is_none",
        default
    )]
    include_unspecified_availability: Option<bool>,
    #[serde(
        rename = "created",
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    created_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "updated",
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    updated_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "lotBiddingOpens",
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    lot_bidding_opens_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "lotScheduledCloses",
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    lot_scheduled_closes_query: Option<RangeQuery<OffsetDateTime>>,
}

impl TryFrom<ProductListingSearchData<String, String, String>> for ProductListingSearch {
    type Error = crate::error::ApiError;

    fn try_from(
        data: ProductListingSearchData<String, String, String>,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            geography: crate::geographic_search::parse_body(data.geography, "search.geography")?,
            shipping: crate::shipping_search::parse(data.shipping, true)?,
            language: data.language,
            currency: data.currency,
            product_listing_query: data.product_listing_query,
            enhanced_search_description: data
                .enhanced_search_description
                .map(EnhancedSearchDescription::try_from)
                .transpose()
                .map_err(|error| {
                    crate::error::ApiError::bad_request(crate::error::BAD_BODY_VALUE)
                        .with_body_field("search.enhancedSearchDescription")
                        .with_detail(error.to_string())
                })?,
            exclude_product_listing_id_query: parse_body_object_ids::<ProductListingId>(
                data.exclude_product_listing_id_query,
                "search.excludeProductListingId",
                "ProductListing",
            )?
            .into(),
            listing_source_id_query: parse_body_object_ids::<ListingSourceId>(
                data.listing_source_id_query,
                "search.listingSourceId",
                "ListingSource",
            )?
            .into(),
            exclude_listing_source_id_query: parse_body_object_ids::<ListingSourceId>(
                data.exclude_listing_source_id_query,
                "search.excludeListingSourceId",
                "ListingSource",
            )?
            .into(),
            auction_id_query: parse_body_object_ids::<AuctionId>(
                data.auction_id_query,
                "search.auctionId",
                "Auction",
            )?
            .into(),
            price_query: data
                .price_query
                .map(|query| query.map(MonetaryAmount::from)),
            availability_query: availability_query_from_parts(
                data.availability_query,
                data.orderability_query,
                data.include_unspecified_availability,
            ),
            created_query: data.created_query,
            updated_query: data.updated_query,
            lot_bidding_opens_query: data.lot_bidding_opens_query,
            lot_scheduled_closes_query: data.lot_scheduled_closes_query,
        })
    }
}

impl From<ProductListingSearch>
    for ProductListingSearchData<ProductListingId, ListingSourceId, AuctionId>
{
    fn from(search: ProductListingSearch) -> Self {
        Self {
            geography: search.geography.map(Into::into),
            shipping: search.shipping.map(Into::into),
            language: search.language,
            currency: search.currency,
            product_listing_query: search.product_listing_query,
            enhanced_search_description: search.enhanced_search_description.map(Into::into),
            exclude_product_listing_id_query: search.exclude_product_listing_id_query.into(),
            listing_source_id_query: search.listing_source_id_query.into(),
            exclude_listing_source_id_query: search.exclude_listing_source_id_query.into(),
            auction_id_query: search.auction_id_query.into(),
            price_query: search.price_query.map(|query| query.map(u64::from)),
            availability_query: search
                .availability_query
                .as_ref()
                .map(|query| query.any_of.iter().copied().collect()),
            orderability_query: search
                .availability_query
                .as_ref()
                .map(|query| query.orderability.iter().copied().collect()),
            include_unspecified_availability: search
                .availability_query
                .as_ref()
                .map(|query| query.include_unspecified),
            created_query: search.created_query,
            updated_query: search.updated_query,
            lot_bidding_opens_query: search.lot_bidding_opens_query,
            lot_scheduled_closes_query: search.lot_scheduled_closes_query,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchFilterData {
    user_id: UserId,
    user_search_filter_id: UserSearchFilterId,
    name: UserSearchFilterName,
    notifications: bool,
    #[serde(with = "crate::wire::search_filter_state")]
    state: SearchFilterState,
    search: ProductListingSearchData<ProductListingId, ListingSourceId, AuctionId>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    created: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
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

fn search_filter_state(state: PatchSearchFilterStateData) -> SearchFilterState {
    match state {
        PatchSearchFilterStateData::Active => SearchFilterState::Active,
        PatchSearchFilterStateData::InactiveByUser => SearchFilterState::InactiveByUser,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchFilterMatchData {
    user_id: UserId,
    user_search_filter_id: UserSearchFilterId,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_search_filter_name: Option<UserSearchFilterName>,
    product_listing_id: ProductListingId,
    origin_event_id: EventId,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_match_valuation: Option<ShippingMatchValuationData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    feedback: Option<bool>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PaginatedData<T> {
    pub(super) items: Vec<T>,
    pub(super) from: u64,
    pub(super) size: u64,
    pub(super) total: Option<u64>,
}

fn geographic_patch(
    value: PatchValue<crate::geographic_search::GeographicSearchData>,
    field: &'static str,
) -> Result<PatchField<geo::core::discovery::GeographicQuery>, crate::error::ApiError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Ok(PatchField::Clear),
        PatchValue::Value(d) => crate::geographic_search::parse_body(Some(d), field)?
            .map(PatchField::Set)
            .ok_or_else(|| crate::error::ApiError::bad_request(crate::error::BAD_BODY_VALUE)),
    }
}

#[cfg(test)]
mod tests;
