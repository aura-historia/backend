use common::distance::data::GeoDistanceQueryData;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::resource_state::data::PatchResourceStateData;
use common::resource_state::domain::ResourceState;
use common::seller_slug_id::SellerSlugId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::{
    currency::{data::CurrencyData, domain::Currency},
    language::{data::LanguageData, domain::Language},
    price::domain::MonetaryAmount,
    product_state::domain::ProductState,
};
use geo::data::continent_data::ContinentData;
use product::data::product_state_data::ProductStateData;
use search_filter::core::user_search_filter_name::UserSearchFilterName;
use search_filter::core::user_search_filter_update::UserSearchFilterUpdate;
use serde::{Deserialize, Serialize};
use shop::core::shop_type::ShopType;
use shop::data::shop_type_data::ShopTypeData;
use std::collections::HashSet;
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PatchUserSearchFilterData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<UserSearchFilterName>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub notifications: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<PatchResourceStateData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub search: Option<PatchProductSearchData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PatchProductSearchData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language: Option<LanguageData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub currency: Option<CurrencyData>,

    #[serde(
        rename = "productQuery",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub product_query: Option<TextQuery<1>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub enhanced_search_description: Option<String>,

    #[serde(rename = "shopName", skip_serializing_if = "Option::is_none", default)]
    pub shop_name_query: Option<HashSet<ShopName>>,

    #[serde(
        rename = "excludeShopName",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub exclude_shop_name_query: Option<HashSet<ShopName>>,

    #[serde(
        rename = "sellerName",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub seller_name_query: Option<HashSet<ShopName>>,

    #[serde(
        rename = "excludeSellerName",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub exclude_seller_name_query: Option<HashSet<ShopName>>,

    #[serde(
        rename = "shopSlugId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub shop_slug_id_query: Option<HashSet<ShopSlugId>>,

    #[serde(
        rename = "excludeShopSlugId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub exclude_shop_slug_id_query: Option<HashSet<ShopSlugId>>,

    #[serde(
        rename = "sellerSlugId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub seller_slug_id_query: Option<HashSet<SellerSlugId>>,

    #[serde(
        rename = "excludeSellerSlugId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub exclude_seller_slug_id_query: Option<HashSet<SellerSlugId>>,

    #[serde(rename = "shopType", skip_serializing_if = "Option::is_none", default)]
    pub shop_type_query: Option<HashSet<ShopTypeData>>,

    #[serde(rename = "country", skip_serializing_if = "Option::is_none", default)]
    pub country_query: Option<HashSet<isocountry::CountryCode>>,

    #[serde(rename = "continent", skip_serializing_if = "Option::is_none", default)]
    pub continent_query: Option<HashSet<ContinentData>>,

    #[serde(
        rename = "geoAddress",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub geo_address_distance_query: Option<GeoDistanceQueryData>,

    #[serde(rename = "price", skip_serializing_if = "Option::is_none", default)]
    pub price_query: Option<RangeQuery<u64>>,

    #[serde(rename = "state", skip_serializing_if = "Option::is_none", default)]
    pub state_query: Option<HashSet<ProductStateData>>,

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

impl From<PatchUserSearchFilterData> for UserSearchFilterUpdate {
    fn from(patch: PatchUserSearchFilterData) -> Self {
        UserSearchFilterUpdate {
            name: patch.name,
            enhanced_search_description: patch
                .search
                .as_ref()
                .and_then(|sf| sf.enhanced_search_description.clone())
                .map(Into::into),
            notifications: patch.notifications,
            state: patch.state.map(ResourceState::from),
            language: patch
                .search
                .as_ref()
                .and_then(|sf| sf.language.map(Language::from)),
            currency: patch
                .search
                .as_ref()
                .and_then(|sf| sf.currency.map(Currency::from)),
            product_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.product_query.clone()),
            shop_name_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.shop_name_query.clone()),
            exclude_shop_name_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.exclude_shop_name_query.clone()),
            seller_name_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.seller_name_query.clone()),
            exclude_seller_name_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.exclude_seller_name_query.clone()),
            shop_slug_id_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.shop_slug_id_query.clone()),
            exclude_shop_slug_id_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.exclude_shop_slug_id_query.clone()),
            seller_slug_id_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.seller_slug_id_query.clone()),
            exclude_seller_slug_id_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.exclude_seller_slug_id_query.clone()),
            shop_type_query: patch.search.as_ref().and_then(|sf| {
                sf.shop_type_query
                    .clone()
                    .map(|types| types.into_iter().map(ShopType::from).collect())
            }),
            country_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.country_query.clone().map(Into::into)),
            continent_query: patch.search.as_ref().and_then(|sf| {
                sf.continent_query
                    .clone()
                    .map(|continents| continents.into_iter().map(Into::into).collect())
            }),
            geo_address_distance_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.geo_address_distance_query.map(Into::into)),
            price_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.price_query.map(|query| query.map(MonetaryAmount::from))),
            state_query: patch.search.as_ref().and_then(|sf| {
                sf.state_query
                    .clone()
                    .map(|states| states.into_iter().map(ProductState::from).collect())
            }),
            created_query: patch.search.as_ref().and_then(|sf| sf.created_query),
            updated_query: patch.search.as_ref().and_then(|sf| sf.updated_query),
            auction_start_query: patch.search.as_ref().and_then(|sf| sf.auction_start_query),
            auction_end_query: patch.search.as_ref().and_then(|sf| sf.auction_end_query),
            updated: OffsetDateTime::now_utc(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};
    use product::core::product_search::faker::fake_range_query_datetime;

    impl Dummy<Faker> for PatchProductSearchData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PatchProductSearchData {
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                product_query: config.fake_with_rng(rng),
                enhanced_search_description: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                exclude_shop_name_query: config.fake_with_rng(rng),
                seller_name_query: config.fake_with_rng(rng),
                exclude_seller_name_query: config.fake_with_rng(rng),
                shop_slug_id_query: config.fake_with_rng(rng),
                exclude_shop_slug_id_query: config.fake_with_rng(rng),
                seller_slug_id_query: config.fake_with_rng(rng),
                exclude_seller_slug_id_query: config.fake_with_rng(rng),
                shop_type_query: config.fake_with_rng(rng),
                country_query: None,
                continent_query: config.fake_with_rng(rng),
                geo_address_distance_query: config.fake_with_rng(rng),
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
                auction_start_query: fake_range_query_datetime(config, rng),
                auction_end_query: fake_range_query_datetime(config, rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::patch_types::{PatchProductSearchData, PatchUserSearchFilterData};
    use common::query::range_query::RangeQuery;
    use common::shop_name::ShopName;
    use common::shop_slug_id::ShopSlugId;
    use common::{currency::data::CurrencyData, language::data::LanguageData};
    use product::data::product_state_data::ProductStateData;
    use serde_json::json;
    use shop::data::shop_type_data::ShopTypeData;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn should_deserialize_search_filter_patch() {
        let json = json!({
            "language": "de",
            "currency": "EUR",
            "productQuery": "Boop",
            "shopName": ["Baap"],
            "shopSlugId": ["imperial-antiques"],
            "price": {
                "min": 37,
                "max": 42
            },
            "state": ["AVAILABLE"],
            "shopType": ["COMMERCIAL_DEALER"],
            "created": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z"
            },
            "updated": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z"
            },
            "auctionStart": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z"
            },
            "auctionEnd": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z"
            }
        });
        let expected = PatchProductSearchData {
            language: Some(LanguageData::De),
            currency: Some(CurrencyData::Eur),
            product_query: Some("Boop".try_into().unwrap()),
            enhanced_search_description: None,
            shop_name_query: Some(HashSet::from_iter([ShopName::from("Baap")])),
            exclude_shop_name_query: None,
            seller_name_query: None,
            exclude_seller_name_query: None,
            shop_slug_id_query: Some(HashSet::from_iter([ShopSlugId::from("imperial-antiques")])),
            exclude_shop_slug_id_query: None,
            seller_slug_id_query: None,
            exclude_seller_slug_id_query: None,
            shop_type_query: Some(HashSet::from_iter([ShopTypeData::CommercialDealer])),
            country_query: None,
            continent_query: None,
            geo_address_distance_query: None,
            price_query: Some(RangeQuery {
                min: Some(37),
                max: Some(42),
            }),
            state_query: Some(HashSet::from_iter([ProductStateData::Available])),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            auction_start_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            auction_end_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
        };

        let actual: PatchProductSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_user_search_filter_patch() {
        let json = json!({
            "name": "hugos filter for peppino",
            "search": {
                "language": "de",
                "currency": "EUR",
                "productQuery": "Boop",
                "enhancedSearchDescription": "I want foo",
                "shopName": ["Baap"],
                "shopSlugId": ["imperial-antiques"],
                "price": {
                    "min": 37,
                    "max": 42
                },
                "state": ["AVAILABLE"],
                "shopType": ["COMMERCIAL_DEALER"],
                "created": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z"
                },
                "updated": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z"
                },
                "auctionStart": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z"
                },
                "auctionEnd": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z"
                }
            }
        });
        let expected = PatchUserSearchFilterData {
            name: Some("hugos filter for peppino".into()),
            notifications: None,
            state: None,
            search: Some(PatchProductSearchData {
                language: Some(LanguageData::De),
                currency: Some(CurrencyData::Eur),
                product_query: Some("Boop".try_into().unwrap()),
                enhanced_search_description: Some("I want foo".into()),
                shop_name_query: Some([ShopName::from("Baap")].into()),
                exclude_shop_name_query: None,
                seller_name_query: None,
                exclude_seller_name_query: None,
                shop_slug_id_query: Some(HashSet::from_iter([ShopSlugId::from(
                    "imperial-antiques",
                )])),
                exclude_shop_slug_id_query: None,
                seller_slug_id_query: None,
                exclude_seller_slug_id_query: None,
                shop_type_query: Some(HashSet::from_iter([ShopTypeData::CommercialDealer])),
                country_query: None,
                continent_query: None,
                geo_address_distance_query: None,
                price_query: Some(RangeQuery {
                    min: Some(37),
                    max: Some(42),
                }),
                state_query: Some(HashSet::from_iter([ProductStateData::Available])),
                created_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
                updated_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
                auction_start_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
                auction_end_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
            }),
        };

        let actual: PatchUserSearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
