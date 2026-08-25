use crate::patch_value::{PatchValue, clearable, non_nullable_option, non_nullable_patch};
use crate::values::GeoDistanceQueryData;
use application::patch_field::PatchField;
use domain_primitives::event_id::EventId;
use domain_primitives::query::any_of_query::AnyOfQuery;
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use localization::Language;
use money::Currency;

use money::MonetaryAmount;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_orderability::ListingOrderability;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use shop_core::seller_slug_id::SellerSlugId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use user_core::user_id::UserId;

use geo::core::continent::Continent;
use geo::data::continent_data::ContinentData;
use isocountry::CountryCode;
use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, EnhancedSearchDescriptionError, ListingAvailabilityQuery,
    ProductListingSearch,
};
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_service::ports::{SearchFilterMatchView, SearchFilterView};
use search_filter_service::use_cases::ProductListingSearchPatch;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use shop_core::shop_type::ShopType;
use std::collections::HashSet;
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
    pub(super) name: PatchValue<UserSearchFilterName>,
    #[serde(default)]
    pub(super) notifications: PatchValue<bool>,
    #[serde(default)]
    pub(super) state: PatchValue<PatchSearchFilterStateData>,
    #[serde(default)]
    pub(super) search: PatchValue<ProductListingSearchPatchData>,
}

impl UpdateSearchFilterData {
    pub(super) fn into_fields(self) -> Result<UpdateSearchFilterFields, crate::error::ApiError> {
        let search = match non_nullable_option(self.search, "search")? {
            Some(search) => search.try_into_patch()?,
            None => ProductListingSearchPatch::default(),
        };

        Ok((
            non_nullable_patch(self.name, "name")?,
            non_nullable_patch(self.notifications, "notifications")?,
            non_nullable_patch(self.state.map(search_filter_state), "state")?,
            search,
        ))
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ProductListingSearchDataMappingError {
    #[error(transparent)]
    EnhancedSearchDescription(#[from] EnhancedSearchDescriptionError),
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProductListingSearchPatchData {
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
    #[serde(rename = "shopName", default)]
    shop_name_query: PatchValue<HashSet<ShopName>>,
    #[serde(rename = "excludeShopName", default)]
    exclude_shop_name_query: PatchValue<HashSet<ShopName>>,
    #[serde(rename = "sellerName", default)]
    seller_name_query: PatchValue<HashSet<ShopName>>,
    #[serde(rename = "excludeSellerName", default)]
    exclude_seller_name_query: PatchValue<HashSet<ShopName>>,
    #[serde(rename = "shopSlugId", default)]
    shop_slug_id_query: PatchValue<HashSet<ShopSlugId>>,
    #[serde(rename = "excludeShopSlugId", default)]
    exclude_shop_slug_id_query: PatchValue<HashSet<ShopSlugId>>,
    #[serde(rename = "sellerSlugId", default)]
    seller_slug_id_query: PatchValue<HashSet<SellerSlugId>>,
    #[serde(rename = "excludeSellerSlugId", default)]
    exclude_seller_slug_id_query: PatchValue<HashSet<SellerSlugId>>,
    #[serde(rename = "shopType", default)]
    #[serde(deserialize_with = "crate::wire::shop_type::patch_set::deserialize")]
    shop_type_query: PatchValue<HashSet<ShopType>>,
    #[serde(rename = "country", default)]
    country_query: PatchValue<HashSet<CountryCode>>,
    #[serde(rename = "continent", default)]
    continent_query: PatchValue<HashSet<ContinentData>>,
    #[serde(rename = "geoAddress", default)]
    geo_address_distance_query: PatchValue<GeoDistanceQueryData>,
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
        rename = "auctionStart",
        default,
        deserialize_with = "crate::patch_value::rfc3339_range::deserialize"
    )]
    auction_start_query: PatchValue<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "auctionEnd",
        default,
        deserialize_with = "crate::patch_value::rfc3339_range::deserialize"
    )]
    auction_end_query: PatchValue<RangeQuery<OffsetDateTime>>,
}

impl ProductListingSearchPatchData {
    fn try_into_patch(self) -> Result<ProductListingSearchPatch, crate::error::ApiError> {
        Ok(ProductListingSearchPatch {
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
            shop_name_query: non_nullable_patch(
                self.shop_name_query.map(AnyOfQuery::from),
                "search.shopName",
            )?,
            exclude_shop_name_query: non_nullable_patch(
                self.exclude_shop_name_query.map(AnyOfQuery::from),
                "search.excludeShopName",
            )?,
            seller_name_query: non_nullable_patch(
                self.seller_name_query.map(AnyOfQuery::from),
                "search.sellerName",
            )?,
            exclude_seller_name_query: non_nullable_patch(
                self.exclude_seller_name_query.map(AnyOfQuery::from),
                "search.excludeSellerName",
            )?,
            shop_slug_id_query: non_nullable_patch(
                self.shop_slug_id_query.map(AnyOfQuery::from),
                "search.shopSlugId",
            )?,
            exclude_shop_slug_id_query: non_nullable_patch(
                self.exclude_shop_slug_id_query.map(AnyOfQuery::from),
                "search.excludeShopSlugId",
            )?,
            seller_slug_id_query: non_nullable_patch(
                self.seller_slug_id_query.map(AnyOfQuery::from),
                "search.sellerSlugId",
            )?,
            exclude_seller_slug_id_query: non_nullable_patch(
                self.exclude_seller_slug_id_query.map(AnyOfQuery::from),
                "search.excludeSellerSlugId",
            )?,
            shop_type_query: non_nullable_patch(
                self.shop_type_query.map(AnyOfQuery::from),
                "search.shopType",
            )?,
            country_query: non_nullable_patch(
                self.country_query.map(AnyOfQuery::from),
                "search.country",
            )?,
            continent_query: non_nullable_patch(
                self.continent_query
                    .map(|values| values.into_iter().map(Continent::from).collect()),
                "search.continent",
            )?,
            geo_address_distance_query: clearable(self.geo_address_distance_query.map(Into::into)),
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
            auction_start_query: clearable(self.auction_start_query),
            auction_end_query: clearable(self.auction_end_query),
        })
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
pub(super) struct ProductListingSearchData {
    #[serde(default)]
    #[serde(with = "crate::wire::language")]
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
        rename = "excludeProductId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    exclude_product_listing_id_query: HashSet<ProductListingId>,
    #[serde(
        rename = "shopName",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    shop_name_query: HashSet<ShopName>,
    #[serde(
        rename = "excludeShopName",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    exclude_shop_name_query: HashSet<ShopName>,
    #[serde(
        rename = "sellerName",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    seller_name_query: HashSet<ShopName>,
    #[serde(
        rename = "excludeSellerName",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    exclude_seller_name_query: HashSet<ShopName>,
    #[serde(
        rename = "shopSlugId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    shop_slug_id_query: HashSet<ShopSlugId>,
    #[serde(
        rename = "excludeShopSlugId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    exclude_shop_slug_id_query: HashSet<ShopSlugId>,
    #[serde(
        rename = "sellerSlugId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    seller_slug_id_query: HashSet<SellerSlugId>,
    #[serde(
        rename = "excludeSellerSlugId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    exclude_seller_slug_id_query: HashSet<SellerSlugId>,
    #[serde(
        rename = "shopType",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    #[serde(with = "crate::wire::shop_type::set")]
    shop_type_query: HashSet<ShopType>,
    #[serde(rename = "country", skip_serializing_if = "HashSet::is_empty", default)]
    country_query: HashSet<CountryCode>,
    #[serde(
        rename = "continent",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    continent_query: HashSet<ContinentData>,
    #[serde(
        rename = "geoAddress",
        skip_serializing_if = "Option::is_none",
        default
    )]
    geo_address_distance_query: Option<GeoDistanceQueryData>,
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
        rename = "auctionStart",
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    auction_start_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "auctionEnd",
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    auction_end_query: Option<RangeQuery<OffsetDateTime>>,
}

impl TryFrom<ProductListingSearchData> for ProductListingSearch {
    type Error = ProductListingSearchDataMappingError;

    fn try_from(data: ProductListingSearchData) -> Result<Self, Self::Error> {
        Ok(Self {
            language: data.language,
            currency: data.currency,
            product_listing_query: data.product_listing_query,
            enhanced_search_description: data
                .enhanced_search_description
                .map(EnhancedSearchDescription::try_from)
                .transpose()?,
            exclude_product_listing_id_query: data.exclude_product_listing_id_query.into(),
            shop_name_query: data.shop_name_query.into(),
            exclude_shop_name_query: data.exclude_shop_name_query.into(),
            seller_name_query: data.seller_name_query.into(),
            exclude_seller_name_query: data.exclude_seller_name_query.into(),
            shop_slug_id_query: data.shop_slug_id_query.into(),
            exclude_shop_slug_id_query: data.exclude_shop_slug_id_query.into(),
            seller_slug_id_query: data.seller_slug_id_query.into(),
            exclude_seller_slug_id_query: data.exclude_seller_slug_id_query.into(),
            shop_type_query: data.shop_type_query.into(),
            country_query: data.country_query.into(),
            continent_query: data.continent_query.into_iter().map(Into::into).collect(),
            geo_address_distance_query: data.geo_address_distance_query.map(Into::into),
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
            auction_start_query: data.auction_start_query,
            auction_end_query: data.auction_end_query,
        })
    }
}

impl From<ProductListingSearch> for ProductListingSearchData {
    fn from(search: ProductListingSearch) -> Self {
        Self {
            language: search.language,
            currency: search.currency,
            product_listing_query: search.product_listing_query,
            enhanced_search_description: search.enhanced_search_description.map(Into::into),
            exclude_product_listing_id_query: search.exclude_product_listing_id_query.into(),
            shop_name_query: search.shop_name_query.into(),
            exclude_shop_name_query: search.exclude_shop_name_query.into(),
            seller_name_query: search.seller_name_query.into(),
            exclude_seller_name_query: search.exclude_seller_name_query.into(),
            shop_slug_id_query: search.shop_slug_id_query.into(),
            exclude_shop_slug_id_query: search.exclude_shop_slug_id_query.into(),
            seller_slug_id_query: search.seller_slug_id_query.into(),
            exclude_seller_slug_id_query: search.exclude_seller_slug_id_query.into(),
            shop_type_query: search.shop_type_query.into(),
            country_query: search.country_query.into(),
            continent_query: search.continent_query.into_iter().map(Into::into).collect(),
            geo_address_distance_query: search.geo_address_distance_query.map(Into::into),
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
            auction_start_query: search.auction_start_query,
            auction_end_query: search.auction_end_query,
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
    search: ProductListingSearchData,
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
    enhanced_match_reason: Option<String>,
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
            enhanced_match_reason: view.enhanced_match_reason.map(String::from),
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

#[cfg(test)]
mod tests {
    use super::*;
    use localization::Language;

    #[test]
    fn should_map_only_supplied_nested_search_fields_to_product_search_patch()
    -> Result<(), Box<dyn std::error::Error>> {
        let data: UpdateSearchFilterData = serde_json::from_str(
            r#"{
                "search": {
                    "language": "de",
                    "productQuery": ["cabinet"],
                    "price": { "min": 10, "max": 20 },
                    "availability": ["AVAILABLE"],
                    "orderability": ["ORDERABLE_NOW"],
                    "includeUnspecifiedAvailability": true
                }
            }"#,
        )?;

        let (_, _, _, patch) = data.into_fields()?;

        assert!(matches!(patch.language, PatchField::Set(Language::De)));
        assert!(matches!(patch.currency, PatchField::Unchanged));
        assert!(matches!(
            patch.enhanced_search_description,
            PatchField::Unchanged
        ));
        let values = match patch.product_listing_query {
            PatchField::Set(values) => values,
            PatchField::Unchanged | PatchField::Clear => {
                return Err(std::io::Error::other("product query was not set").into());
            }
        };
        assert_eq!("cabinet", values[0].as_ref());
        assert!(matches!(patch.price_query, PatchField::Set(_)));
        assert_eq!(
            PatchField::Set(ListingAvailabilityQuery {
                any_of: HashSet::from([ListingAvailability::Available]).into(),
                orderability: HashSet::from([ListingOrderability::OrderableNow]).into(),
                include_unspecified: true,
            }),
            patch.availability_query
        );
        assert!(matches!(patch.shop_name_query, PatchField::Unchanged));
        Ok(())
    }

    #[test]
    fn should_preserve_absent_and_configured_empty_availability_queries()
    -> Result<(), Box<dyn std::error::Error>> {
        let absent =
            ProductListingSearchData::from(ProductListingSearch::new(Language::En, Currency::Eur));
        let absent_value = serde_json::to_value(absent)?;
        assert!(absent_value.get("availability").is_none());
        assert!(absent_value.get("orderability").is_none());
        assert!(absent_value.get("includeUnspecifiedAvailability").is_none());

        let mut configured_empty = ProductListingSearch::new(Language::En, Currency::Eur);
        configured_empty.availability_query = Some(ListingAvailabilityQuery {
            any_of: Default::default(),
            orderability: Default::default(),
            include_unspecified: false,
        });
        let value = serde_json::to_value(ProductListingSearchData::from(configured_empty))?;
        assert_eq!(Some(&serde_json::json!([])), value.get("availability"));
        assert_eq!(Some(&serde_json::json!([])), value.get("orderability"));
        assert_eq!(
            Some(&serde_json::json!(false)),
            value.get("includeUnspecifiedAvailability")
        );
        let data: ProductListingSearchData = serde_json::from_value(value)?;
        let decoded = ProductListingSearch::try_from(data)?;
        assert_eq!(
            Some(ListingAvailabilityQuery {
                any_of: Default::default(),
                orderability: Default::default(),
                include_unspecified: false,
            }),
            decoded.availability_query
        );
        Ok(())
    }

    #[test]
    fn should_require_an_atomic_availability_query_patch() -> Result<(), Box<dyn std::error::Error>>
    {
        let clear: UpdateSearchFilterData = serde_json::from_str(
            r#"{ "search": {
                "availability": null,
                "orderability": null,
                "includeUnspecifiedAvailability": null
            } }"#,
        )?;
        let (_, _, _, patch) = clear.into_fields()?;
        assert_eq!(PatchField::Clear, patch.availability_query);

        let partial: UpdateSearchFilterData =
            serde_json::from_str(r#"{ "search": { "orderability": null } }"#)?;
        assert!(partial.into_fields().is_err());

        let omitted: UpdateSearchFilterData = serde_json::from_str(r#"{ "search": {} }"#)?;
        let (_, _, _, patch) = omitted.into_fields()?;
        assert_eq!(PatchField::Unchanged, patch.availability_query);
        Ok(())
    }
}
