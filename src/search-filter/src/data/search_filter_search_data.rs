use common::distance::data::GeoDistanceQueryData;
use common::query::{range_query::RangeQuery, text_query::TextQuery};
use common::shop_name::ShopName;
use common::slug_id::SlugId;
use common::{currency::data::CurrencyData, language::data::LanguageData};
use geo::{core::continent::Continent, data::continent_data::ContinentData};
use isocountry::CountryCode;
use product::core::product_search::ProductSearch;
use product::data::product_state_data::ProductStateData;
use serde::{Deserialize, Serialize};
use shop::{core::shop_type::ShopType, data::shop_type_data::ShopTypeData};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilterSearchData {
    pub language: LanguageData,
    pub currency: CurrencyData,

    #[serde(
        rename = "productQuery",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub product_query: Option<TextQuery<1>>,

    #[serde(
        rename = "shopName",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub shop_name_query: HashSet<ShopName>,

    #[serde(
        rename = "excludeShopName",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub exclude_shop_name_query: HashSet<ShopName>,

    #[serde(
        rename = "sellerName",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub seller_name_query: HashSet<ShopName>,

    #[serde(
        rename = "excludeSellerName",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub exclude_seller_name_query: HashSet<ShopName>,

    #[serde(
        rename = "shopSlugId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub shop_slug_id_query: HashSet<SlugId<0>>,

    #[serde(
        rename = "excludeShopSlugId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub exclude_shop_slug_id_query: HashSet<SlugId<0>>,

    #[serde(
        rename = "sellerSlugId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub seller_slug_id_query: HashSet<SlugId<0>>,

    #[serde(
        rename = "excludeSellerSlugId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub exclude_seller_slug_id_query: HashSet<SlugId<0>>,

    #[serde(
        rename = "shopType",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub shop_type_query: HashSet<ShopTypeData>,

    #[serde(rename = "country", skip_serializing_if = "HashSet::is_empty", default)]
    pub country_query: HashSet<CountryCode>,

    #[serde(
        rename = "continent",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub continent_query: HashSet<ContinentData>,

    #[serde(
        rename = "geoAddress",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub geo_address_distance_query: Option<GeoDistanceQueryData>,

    #[serde(rename = "price", skip_serializing_if = "Option::is_none", default)]
    pub price_query: Option<RangeQuery<u64>>,

    #[serde(rename = "state", skip_serializing_if = "HashSet::is_empty", default)]
    pub state_query: HashSet<ProductStateData>,

    #[serde(
        rename = "created",
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub created_query: Option<RangeQuery<OffsetDateTime>>,

    #[serde(
        rename = "updated",
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,

    #[serde(
        rename = "auctionStart",
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub auction_start_query: Option<RangeQuery<OffsetDateTime>>,

    #[serde(
        rename = "auctionEnd",
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub auction_end_query: Option<RangeQuery<OffsetDateTime>>,
}

impl From<ProductSearch> for SearchFilterSearchData {
    fn from(search: ProductSearch) -> Self {
        SearchFilterSearchData {
            language: search.language.into(),
            currency: search.currency.into(),
            product_query: search.product_query,
            shop_name_query: search.shop_name_query.into(),
            exclude_shop_name_query: search.exclude_shop_name_query.into(),
            seller_name_query: search.seller_name_query.into(),
            exclude_seller_name_query: search.exclude_seller_name_query.into(),
            shop_slug_id_query: search.shop_slug_id_query.into(),
            exclude_shop_slug_id_query: search.exclude_shop_slug_id_query.into(),
            seller_slug_id_query: search.seller_slug_id_query.into(),
            exclude_seller_slug_id_query: search.exclude_seller_slug_id_query.into(),
            shop_type_query: search
                .shop_type_query
                .into_iter()
                .map(ShopTypeData::from)
                .collect(),
            country_query: search.country_query.into(),
            continent_query: search
                .continent_query
                .into_iter()
                .map(ContinentData::from)
                .collect(),
            geo_address_distance_query: search.geo_address_distance_query.map(Into::into),
            price_query: search.price_query.map(|range| range.map(u64::from)),
            state_query: search
                .state_query
                .into_iter()
                .map(ProductStateData::from)
                .collect(),
            created_query: search.created_query,
            updated_query: search.updated_query,
            auction_start_query: search.auction_start_query,
            auction_end_query: search.auction_end_query,
        }
    }
}

impl From<SearchFilterSearchData> for ProductSearch {
    fn from(search: SearchFilterSearchData) -> Self {
        ProductSearch {
            language: search.language.into(),
            currency: search.currency.into(),
            product_query: search.product_query,
            shop_name_query: search.shop_name_query.into(),
            exclude_shop_name_query: search.exclude_shop_name_query.into(),
            seller_name_query: search.seller_name_query.into(),
            exclude_seller_name_query: search.exclude_seller_name_query.into(),
            shop_slug_id_query: search.shop_slug_id_query.into(),
            exclude_shop_slug_id_query: search.exclude_shop_slug_id_query.into(),
            seller_slug_id_query: search.seller_slug_id_query.into(),
            exclude_seller_slug_id_query: search.exclude_seller_slug_id_query.into(),
            shop_type_query: search
                .shop_type_query
                .into_iter()
                .map(ShopType::from)
                .collect(),
            country_query: search.country_query.into(),
            continent_query: search
                .continent_query
                .into_iter()
                .map(Continent::from)
                .collect(),
            geo_address_distance_query: search.geo_address_distance_query.map(Into::into),
            price_query: search.price_query.map(|range| range.map(Into::into)),
            state_query: search.state_query.into_iter().map(Into::into).collect(),
            created_query: search.created_query,
            updated_query: search.updated_query,
            auction_start_query: search.auction_start_query,
            auction_end_query: search.auction_end_query,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for SearchFilterSearchData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<ProductSearch, _>(rng).into()
        }
    }
}
