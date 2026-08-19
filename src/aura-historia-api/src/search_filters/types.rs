use crate::values::{CurrencyData, GeoDistanceQueryData, LanguageData};
use common::event_id::EventId;
use common::patch_field::PatchField;
use common::product_id::ProductId;
use common::product_state::domain::ProductState;
use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::resource_state::data::{PatchResourceStateData, ResourceStateData};
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use money::MonetaryAmount;
use shop_core::seller_slug_id::SellerSlugId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;

use geo::core::continent::Continent;
use geo::data::continent_data::ContinentData;
use isocountry::CountryCode;
use product_core::product_search::{EnhancedSearchDescription, ProductSearch};
use search_filter_service::ports::{SearchFilterMatchView, SearchFilterView};
use search_filter_service::use_cases::ProductSearchPatch;
use serde::{Deserialize, Serialize};
use shop_core::shop_type::ShopType;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateSearchFilterData {
    pub(super) name: UserSearchFilterName,
    #[serde(default = "default_notifications")]
    pub(super) notifications: bool,
    pub(super) search: ProductSearchData,
}

fn default_notifications() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateSearchFilterData {
    #[serde(default)]
    pub(super) name: Option<UserSearchFilterName>,
    #[serde(default)]
    pub(super) notifications: Option<bool>,
    #[serde(default)]
    pub(super) state: Option<PatchResourceStateData>,
    #[serde(default)]
    pub(super) search: Option<ProductSearchPatchData>,
}

impl UpdateSearchFilterData {
    pub(super) fn into_fields(
        self,
    ) -> (
        PatchField<UserSearchFilterName>,
        PatchField<bool>,
        PatchField<common::resource_state::domain::ResourceState>,
        ProductSearchPatch,
    ) {
        (
            patch(self.name),
            patch(self.notifications),
            patch(self.state.map(Into::into)),
            self.search.map(Into::into).unwrap_or_default(),
        )
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProductSearchPatchData {
    #[serde(default)]
    language: Option<LanguageData>,
    #[serde(default)]
    currency: Option<CurrencyData>,
    #[serde(rename = "productQuery", default)]
    product_query: Option<Vec<TextQuery<1>>>,
    #[serde(rename = "enhancedSearchDescription", default)]
    enhanced_search_description: Option<String>,
    #[serde(rename = "shopName", default)]
    shop_name_query: Option<HashSet<ShopName>>,
    #[serde(rename = "excludeShopName", default)]
    exclude_shop_name_query: Option<HashSet<ShopName>>,
    #[serde(rename = "sellerName", default)]
    seller_name_query: Option<HashSet<ShopName>>,
    #[serde(rename = "excludeSellerName", default)]
    exclude_seller_name_query: Option<HashSet<ShopName>>,
    #[serde(rename = "shopSlugId", default)]
    shop_slug_id_query: Option<HashSet<ShopSlugId>>,
    #[serde(rename = "excludeShopSlugId", default)]
    exclude_shop_slug_id_query: Option<HashSet<ShopSlugId>>,
    #[serde(rename = "sellerSlugId", default)]
    seller_slug_id_query: Option<HashSet<SellerSlugId>>,
    #[serde(rename = "excludeSellerSlugId", default)]
    exclude_seller_slug_id_query: Option<HashSet<SellerSlugId>>,
    #[serde(rename = "shopType", default)]
    shop_type_query: Option<HashSet<ShopTypeData>>,
    #[serde(rename = "country", default)]
    country_query: Option<HashSet<CountryCode>>,
    #[serde(rename = "continent", default)]
    continent_query: Option<HashSet<ContinentData>>,
    #[serde(rename = "geoAddress", default)]
    geo_address_distance_query: Option<GeoDistanceQueryData>,
    #[serde(rename = "price", default)]
    price_query: Option<RangeQuery<u64>>,
    #[serde(rename = "state", default)]
    state_query: Option<HashSet<ProductStateData>>,
    #[serde(
        rename = "created",
        with = "common::query::range_query::range_rfc3339::option",
        default
    )]
    created_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "updated",
        with = "common::query::range_query::range_rfc3339::option",
        default
    )]
    updated_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "auctionStart",
        with = "common::query::range_query::range_rfc3339::option",
        default
    )]
    auction_start_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "auctionEnd",
        with = "common::query::range_query::range_rfc3339::option",
        default
    )]
    auction_end_query: Option<RangeQuery<OffsetDateTime>>,
}

impl From<ProductSearchPatchData> for ProductSearchPatch {
    fn from(data: ProductSearchPatchData) -> Self {
        Self {
            language: patch(data.language.map(Into::into)),
            currency: patch(data.currency.map(Into::into)),
            product_query: patch(data.product_query),
            enhanced_search_description: patch(
                data.enhanced_search_description
                    .map(EnhancedSearchDescription::from),
            ),
            shop_name_query: patch(data.shop_name_query.map(AnyOfQuery::from)),
            exclude_shop_name_query: patch(data.exclude_shop_name_query.map(AnyOfQuery::from)),
            seller_name_query: patch(data.seller_name_query.map(AnyOfQuery::from)),
            exclude_seller_name_query: patch(data.exclude_seller_name_query.map(AnyOfQuery::from)),
            shop_slug_id_query: patch(data.shop_slug_id_query.map(AnyOfQuery::from)),
            exclude_shop_slug_id_query: patch(
                data.exclude_shop_slug_id_query.map(AnyOfQuery::from),
            ),
            seller_slug_id_query: patch(data.seller_slug_id_query.map(AnyOfQuery::from)),
            exclude_seller_slug_id_query: patch(
                data.exclude_seller_slug_id_query.map(AnyOfQuery::from),
            ),
            shop_type_query: patch(
                data.shop_type_query
                    .map(|values| values.into_iter().map(ShopType::from).collect()),
            ),
            country_query: patch(data.country_query.map(AnyOfQuery::from)),
            continent_query: patch(
                data.continent_query
                    .map(|values| values.into_iter().map(Continent::from).collect()),
            ),
            geo_address_distance_query: patch(data.geo_address_distance_query.map(Into::into)),
            price_query: patch(
                data.price_query
                    .map(|query| query.map(MonetaryAmount::from)),
            ),
            state_query: patch(
                data.state_query
                    .map(|values| values.into_iter().map(ProductState::from).collect()),
            ),
            created_query: patch(data.created_query),
            updated_query: patch(data.updated_query),
            auction_start_query: patch(data.auction_start_query),
            auction_end_query: patch(data.auction_end_query),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateSearchFilterMatchFeedbackData {
    #[serde(default)]
    pub(super) feedback: Option<bool>,
}

impl UpdateSearchFilterMatchFeedbackData {
    pub(super) fn feedback(self) -> PatchField<bool> {
        patch(self.feedback)
    }
}

fn patch<T>(value: Option<T>) -> PatchField<T> {
    value.map(PatchField::Set).unwrap_or(PatchField::Unchanged)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProductSearchData {
    #[serde(default)]
    language: LanguageData,
    #[serde(default)]
    currency: CurrencyData,
    #[serde(
        rename = "productQuery",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    product_query: Vec<TextQuery<1>>,
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
    exclude_product_id_query: HashSet<ProductId>,
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
    shop_type_query: HashSet<ShopTypeData>,
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
    #[serde(rename = "state", skip_serializing_if = "HashSet::is_empty", default)]
    state_query: HashSet<ProductStateData>,
    #[serde(
        rename = "created",
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    created_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "updated",
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    updated_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "auctionStart",
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    auction_start_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "auctionEnd",
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    auction_end_query: Option<RangeQuery<OffsetDateTime>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductStateData {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ShopTypeData {
    AuctionHouse,
    AuctionPlatform,
    CommercialDealer,
    Marketplace,
}

impl From<ProductSearchData> for ProductSearch {
    fn from(data: ProductSearchData) -> Self {
        Self {
            language: data.language.into(),
            currency: data.currency.into(),
            product_query: data.product_query,
            enhanced_search_description: data
                .enhanced_search_description
                .map(EnhancedSearchDescription::from),
            exclude_product_id_query: data.exclude_product_id_query.into(),
            shop_name_query: data.shop_name_query.into(),
            exclude_shop_name_query: data.exclude_shop_name_query.into(),
            seller_name_query: data.seller_name_query.into(),
            exclude_seller_name_query: data.exclude_seller_name_query.into(),
            shop_slug_id_query: data.shop_slug_id_query.into(),
            exclude_shop_slug_id_query: data.exclude_shop_slug_id_query.into(),
            seller_slug_id_query: data.seller_slug_id_query.into(),
            exclude_seller_slug_id_query: data.exclude_seller_slug_id_query.into(),
            shop_type_query: data.shop_type_query.into_iter().map(Into::into).collect(),
            country_query: data.country_query.into(),
            continent_query: data.continent_query.into_iter().map(Into::into).collect(),
            geo_address_distance_query: data.geo_address_distance_query.map(Into::into),
            price_query: data
                .price_query
                .map(|query| query.map(MonetaryAmount::from)),
            state_query: data.state_query.into_iter().map(Into::into).collect(),
            lifecycle_query: Default::default(),
            created_query: data.created_query,
            updated_query: data.updated_query,
            auction_start_query: data.auction_start_query,
            auction_end_query: data.auction_end_query,
        }
    }
}

impl From<ProductSearch> for ProductSearchData {
    fn from(search: ProductSearch) -> Self {
        Self {
            language: search.language.into(),
            currency: search.currency.into(),
            product_query: search.product_query,
            enhanced_search_description: search.enhanced_search_description.map(Into::into),
            exclude_product_id_query: search.exclude_product_id_query.into(),
            shop_name_query: search.shop_name_query.into(),
            exclude_shop_name_query: search.exclude_shop_name_query.into(),
            seller_name_query: search.seller_name_query.into(),
            exclude_seller_name_query: search.exclude_seller_name_query.into(),
            shop_slug_id_query: search.shop_slug_id_query.into(),
            exclude_shop_slug_id_query: search.exclude_shop_slug_id_query.into(),
            seller_slug_id_query: search.seller_slug_id_query.into(),
            exclude_seller_slug_id_query: search.exclude_seller_slug_id_query.into(),
            shop_type_query: search.shop_type_query.into_iter().map(Into::into).collect(),
            country_query: search.country_query.into(),
            continent_query: search.continent_query.into_iter().map(Into::into).collect(),
            geo_address_distance_query: search.geo_address_distance_query.map(Into::into),
            price_query: search.price_query.map(|query| query.map(u64::from)),
            state_query: search.state_query.into_iter().map(Into::into).collect(),
            created_query: search.created_query,
            updated_query: search.updated_query,
            auction_start_query: search.auction_start_query,
            auction_end_query: search.auction_end_query,
        }
    }
}

impl From<ProductStateData> for ProductState {
    fn from(value: ProductStateData) -> Self {
        match value {
            ProductStateData::Listed => Self::Listed,
            ProductStateData::Available => Self::Available,
            ProductStateData::Reserved => Self::Reserved,
            ProductStateData::Sold => Self::Sold,
            ProductStateData::Removed => Self::Removed,
            ProductStateData::Unknown => Self::Unknown,
        }
    }
}
impl From<ProductState> for ProductStateData {
    fn from(value: ProductState) -> Self {
        match value {
            ProductState::Listed => Self::Listed,
            ProductState::Available => Self::Available,
            ProductState::Reserved => Self::Reserved,
            ProductState::Sold => Self::Sold,
            ProductState::Removed => Self::Removed,
            ProductState::Unknown => Self::Unknown,
        }
    }
}
impl From<ShopTypeData> for ShopType {
    fn from(value: ShopTypeData) -> Self {
        match value {
            ShopTypeData::AuctionHouse => Self::AuctionHouse,
            ShopTypeData::AuctionPlatform => Self::AuctionPlatform,
            ShopTypeData::CommercialDealer => Self::CommercialDealer,
            ShopTypeData::Marketplace => Self::Marketplace,
        }
    }
}
impl From<ShopType> for ShopTypeData {
    fn from(value: ShopType) -> Self {
        match value {
            ShopType::AuctionHouse => Self::AuctionHouse,
            ShopType::AuctionPlatform => Self::AuctionPlatform,
            ShopType::CommercialDealer => Self::CommercialDealer,
            ShopType::Marketplace => Self::Marketplace,
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
    state: ResourceStateData,
    search: ProductSearchData,
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
            state: view.state.into(),
            search: view.search.into(),
            created: Some(view.created),
            updated: Some(view.updated),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchFilterMatchData {
    user_id: UserId,
    user_search_filter_id: UserSearchFilterId,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_search_filter_name: Option<UserSearchFilterName>,
    product_id: ProductId,
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
            product_id: view.product_id,
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

        let (_, _, _, patch) = data.into_fields();

        assert!(matches!(patch.language, PatchField::Set(Language::De)));
        assert!(matches!(patch.currency, PatchField::Unchanged));
        assert!(matches!(
            patch.enhanced_search_description,
            PatchField::Unchanged
        ));
        let values = match patch.product_query {
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
