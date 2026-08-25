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
    EnhancedSearchDescription, EnhancedSearchDescriptionError, ProductListingSearch,
};
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_service::ports::{SearchFilterMatchView, SearchFilterView};
use search_filter_service::use_cases::ProductListingSearchPatch;
use serde::{Deserialize, Serialize};
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
            availability_query: non_nullable_patch(
                self.availability_query.map(AnyOfQuery::from),
                "search.availability",
            )?,
            created_query: clearable(self.created_query),
            updated_query: clearable(self.updated_query),
            auction_start_query: clearable(self.auction_start_query),
            auction_end_query: clearable(self.auction_end_query),
        })
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
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    #[serde(with = "crate::wire::listing_availability::set")]
    availability_query: HashSet<ListingAvailability>,
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
            availability_query: data.availability_query.into(),
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
            availability_query: search.availability_query.into(),
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
                    "state": ["AVAILABLE"]
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
        assert!(matches!(patch.state_query, PatchField::Set(_)));
        assert!(matches!(patch.shop_name_query, PatchField::Unchanged));
        Ok(())
    }
}
