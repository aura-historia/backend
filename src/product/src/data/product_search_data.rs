use crate::core::product_search::{EnhancedSearchDescription, ProductSearch};
use crate::data::product_state_data::ProductStateData;
use common::distance::data::GeoDistanceQueryData;
use common::product_id::ProductId;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::seller_slug_id::SellerSlugId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::{
    currency::data::CurrencyData, language::data::LanguageData, price::domain::MonetaryAmount,
    product_state::domain::ProductState,
};
use geo::core::continent::Continent;
use geo::data::continent_data::ContinentData;
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use shop::core::shop_type::ShopType;
use shop::data::shop_type_data::ShopTypeData;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductSearchData {
    #[serde(default)]
    pub language: LanguageData,
    #[serde(default)]
    pub currency: CurrencyData,
    #[serde(
        rename = "productQuery",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub product_query: Vec<TextQuery<1>>,
    #[serde(
        rename = "enhancedSearchDescription",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub enhanced_search_description: Option<String>,
    #[serde(
        rename = "excludeProductId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub exclude_product_id_query: HashSet<ProductId>,
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
    pub shop_slug_id_query: HashSet<ShopSlugId>,
    #[serde(
        rename = "excludeShopSlugId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub exclude_shop_slug_id_query: HashSet<ShopSlugId>,
    #[serde(
        rename = "sellerSlugId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub seller_slug_id_query: HashSet<SellerSlugId>,
    #[serde(
        rename = "excludeSellerSlugId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub exclude_seller_slug_id_query: HashSet<SellerSlugId>,
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

impl From<ProductSearch> for ProductSearchData {
    fn from(search_filter: ProductSearch) -> Self {
        ProductSearchData {
            language: search_filter.language.into(),
            currency: search_filter.currency.into(),
            product_query: search_filter.product_query,
            enhanced_search_description: search_filter.enhanced_search_description.map(Into::into),
            exclude_product_id_query: search_filter.exclude_product_id_query.into(),
            shop_name_query: search_filter.shop_name_query.into(),
            exclude_shop_name_query: search_filter.exclude_shop_name_query.into(),
            seller_name_query: search_filter.seller_name_query.into(),
            exclude_seller_name_query: search_filter.exclude_seller_name_query.into(),
            shop_slug_id_query: search_filter.shop_slug_id_query.into(),
            exclude_shop_slug_id_query: search_filter.exclude_shop_slug_id_query.into(),
            seller_slug_id_query: search_filter.seller_slug_id_query.into(),
            exclude_seller_slug_id_query: search_filter.exclude_seller_slug_id_query.into(),
            shop_type_query: search_filter
                .shop_type_query
                .into_iter()
                .map(ShopTypeData::from)
                .collect(),
            country_query: search_filter.country_query.into(),
            continent_query: search_filter
                .continent_query
                .into_iter()
                .map(ContinentData::from)
                .collect(),
            geo_address_distance_query: search_filter.geo_address_distance_query.map(Into::into),
            price_query: search_filter
                .price_query
                .map(|price_query| price_query.map(u64::from)),
            state_query: search_filter
                .state_query
                .into_iter()
                .map(ProductStateData::from)
                .collect(),
            created_query: search_filter.created_query,
            updated_query: search_filter.updated_query,
            auction_start_query: search_filter.auction_start_query,
            auction_end_query: search_filter.auction_end_query,
        }
    }
}

impl From<ProductSearchData> for ProductSearch {
    fn from(data: ProductSearchData) -> Self {
        ProductSearch {
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
            shop_type_query: data
                .shop_type_query
                .into_iter()
                .map(ShopType::from)
                .collect(),
            country_query: data.country_query.into(),
            continent_query: data
                .continent_query
                .into_iter()
                .map(Continent::from)
                .collect(),
            geo_address_distance_query: data.geo_address_distance_query.map(Into::into),
            price_query: data
                .price_query
                .map(|query| query.map(MonetaryAmount::from)),
            state_query: data
                .state_query
                .into_iter()
                .map(ProductState::from)
                .collect(),
            created_query: data.created_query,
            updated_query: data.updated_query,
            auction_start_query: data.auction_start_query,
            auction_end_query: data.auction_end_query,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductSearchData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<ProductSearch, _>(rng).into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::data::product_search_data::ProductSearchData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_search_data() {
            let _ = Faker.fake::<ProductSearchData>();
        }
    }
}
