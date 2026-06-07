use crate::core::product_search::ProductSearch;
use crate::opensearch::product_state_document::ProductStateDocument;
use common::distance::domain::{Distance, DistanceUnit, GeoDistanceQuery};
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::seller_slug_id::SellerSlugId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::{
    currency::record::CurrencyRecord, language::record::LanguageRecord,
    price::domain::MonetaryAmount,
};
use geo::core::continent::Continent;
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use shop::{
    core::shop_type::ShopType,
    opensearch::{continent_document::ContinentDocument, shop_type_document::ShopTypeDocument},
};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DistanceDocument {
    pub amount: f64,
    pub unit: DistanceUnitDocument,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoDistanceQueryDocument {
    pub lat: f64,
    pub lon: f64,
    pub distance: DistanceDocument,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DistanceUnitDocument {
    Miles,
    Yards,
    Feet,
    Inches,
    Kilometers,
    Meters,
    Centimeters,
    Millimeters,
    NauticalMiles,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductSearchDocument {
    pub language: LanguageRecord,
    pub currency: CurrencyRecord,
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
    pub shop_type_query: HashSet<ShopTypeDocument>,
    #[serde(rename = "country", skip_serializing_if = "HashSet::is_empty", default)]
    pub country_query: HashSet<CountryCode>,
    #[serde(
        rename = "continent",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub continent_query: HashSet<ContinentDocument>,
    #[serde(
        rename = "geoAddress",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub geo_address_distance_query: Option<GeoDistanceQueryDocument>,
    #[serde(rename = "price", skip_serializing_if = "Option::is_none", default)]
    pub price_query: Option<RangeQuery<u64>>,
    #[serde(rename = "state", skip_serializing_if = "HashSet::is_empty", default)]
    pub state_query: HashSet<ProductStateDocument>,
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

impl From<ProductSearch> for ProductSearchDocument {
    fn from(search: ProductSearch) -> Self {
        Self {
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
                .map(ShopTypeDocument::from)
                .collect(),
            country_query: search.country_query.into(),
            continent_query: search
                .continent_query
                .into_iter()
                .map(ContinentDocument::from)
                .collect(),
            geo_address_distance_query: search.geo_address_distance_query.map(Into::into),
            price_query: search
                .price_query
                .map(|range_query| range_query.map(u64::from)),
            state_query: search
                .state_query
                .into_iter()
                .map(ProductStateDocument::from)
                .collect(),
            created_query: search.created_query,
            updated_query: search.updated_query,
            auction_start_query: search.auction_start_query,
            auction_end_query: search.auction_end_query,
        }
    }
}

impl From<ProductSearchDocument> for ProductSearch {
    fn from(document: ProductSearchDocument) -> Self {
        Self {
            language: document.language.into(),
            currency: document.currency.into(),
            product_query: document.product_query,
            shop_name_query: document.shop_name_query.into(),
            exclude_shop_name_query: document.exclude_shop_name_query.into(),
            seller_name_query: document.seller_name_query.into(),
            exclude_seller_name_query: document.exclude_seller_name_query.into(),
            shop_slug_id_query: document.shop_slug_id_query.into(),
            exclude_shop_slug_id_query: document.exclude_shop_slug_id_query.into(),
            seller_slug_id_query: document.seller_slug_id_query.into(),
            exclude_seller_slug_id_query: document.exclude_seller_slug_id_query.into(),
            shop_type_query: document
                .shop_type_query
                .into_iter()
                .map(ShopType::from)
                .collect(),
            country_query: document.country_query.into(),
            continent_query: document
                .continent_query
                .into_iter()
                .map(Continent::from)
                .collect(),
            geo_address_distance_query: document.geo_address_distance_query.map(Into::into),
            price_query: document
                .price_query
                .map(|query| query.map(MonetaryAmount::from)),
            state_query: document.state_query.into_iter().map(Into::into).collect(),
            created_query: document.created_query,
            updated_query: document.updated_query,
            auction_start_query: document.auction_start_query,
            auction_end_query: document.auction_end_query,
        }
    }
}

impl From<GeoDistanceQuery> for GeoDistanceQueryDocument {
    fn from(query: GeoDistanceQuery) -> Self {
        Self {
            lat: query.lat,
            lon: query.lon,
            distance: query.distance.into(),
        }
    }
}

impl From<GeoDistanceQueryDocument> for GeoDistanceQuery {
    fn from(document: GeoDistanceQueryDocument) -> Self {
        Self {
            lat: document.lat,
            lon: document.lon,
            distance: document.distance.into(),
        }
    }
}

impl From<Distance> for DistanceDocument {
    fn from(distance: Distance) -> Self {
        Self {
            amount: distance.amount,
            unit: distance.unit.into(),
        }
    }
}

impl From<DistanceDocument> for Distance {
    fn from(document: DistanceDocument) -> Self {
        Self {
            amount: document.amount,
            unit: document.unit.into(),
        }
    }
}

impl From<DistanceUnit> for DistanceUnitDocument {
    fn from(unit: DistanceUnit) -> Self {
        match unit {
            DistanceUnit::Miles => Self::Miles,
            DistanceUnit::Yards => Self::Yards,
            DistanceUnit::Feet => Self::Feet,
            DistanceUnit::Inches => Self::Inches,
            DistanceUnit::Kilometers => Self::Kilometers,
            DistanceUnit::Meters => Self::Meters,
            DistanceUnit::Centimeters => Self::Centimeters,
            DistanceUnit::Millimeters => Self::Millimeters,
            DistanceUnit::NauticalMiles => Self::NauticalMiles,
        }
    }
}

impl From<DistanceUnitDocument> for DistanceUnit {
    fn from(unit: DistanceUnitDocument) -> Self {
        match unit {
            DistanceUnitDocument::Miles => Self::Miles,
            DistanceUnitDocument::Yards => Self::Yards,
            DistanceUnitDocument::Feet => Self::Feet,
            DistanceUnitDocument::Inches => Self::Inches,
            DistanceUnitDocument::Kilometers => Self::Kilometers,
            DistanceUnitDocument::Meters => Self::Meters,
            DistanceUnitDocument::Centimeters => Self::Centimeters,
            DistanceUnitDocument::Millimeters => Self::Millimeters,
            DistanceUnitDocument::NauticalMiles => Self::NauticalMiles,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProductSearchDocument;
    use crate::core::product_search::ProductSearch;
    use fake::{Fake, Faker};

    #[test]
    fn should_roundtrip_product_search_document() {
        let search = Faker.fake::<ProductSearch>();

        let document = ProductSearchDocument::from(search.clone());

        assert_eq!(ProductSearch::from(document), search);
    }
}
