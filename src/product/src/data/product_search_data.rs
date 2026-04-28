use crate::core::authenticity::Authenticity;
use crate::core::condition::Condition;
use crate::core::product_search::ProductSearch;
use crate::core::provenance::Provenance;
use crate::core::restoration::Restoration;
use crate::data::authenticity_data::AuthenticityData;
use crate::data::condition_data::ConditionData;
use crate::data::product_state_data::ProductStateData;
use crate::data::provenance_data::ProvenanceData;
use crate::data::restoration_data::RestorationData;
use common::category_key::CategoryId;
use common::distance::data::GeoDistanceQueryData;
use common::period_key::PeriodId;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::shop_name::ShopName;
use common::slug_id::SlugId;
use common::year::Year;
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
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub product_query: Option<TextQuery<1>>,
    #[serde(
        rename = "categoryId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub category_id: HashSet<CategoryId>,
    #[serde(
        rename = "periodId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub period_id: HashSet<PeriodId>,
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
        rename = "originYear",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub origin_year_query: Option<RangeQuery<Year>>,
    #[serde(
        rename = "authenticity",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub authenticity_query: HashSet<AuthenticityData>,
    #[serde(
        rename = "condition",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub condition_query: HashSet<ConditionData>,
    #[serde(
        rename = "provenance",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub provenance_query: HashSet<ProvenanceData>,
    #[serde(
        rename = "restoration",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub restoration_query: HashSet<RestorationData>,

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
            category_id: search_filter.category_id.into(),
            period_id: search_filter.period_id.into(),
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
            origin_year_query: search_filter.origin_year_query,
            authenticity_query: search_filter
                .authenticity_query
                .into_iter()
                .map(AuthenticityData::from)
                .collect(),
            condition_query: search_filter
                .condition_query
                .into_iter()
                .map(ConditionData::from)
                .collect(),
            provenance_query: search_filter
                .provenance_query
                .into_iter()
                .map(ProvenanceData::from)
                .collect(),
            restoration_query: search_filter
                .restoration_query
                .into_iter()
                .map(RestorationData::from)
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
            category_id: data.category_id.into(),
            period_id: data.period_id.into(),
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
            origin_year_query: data.origin_year_query,
            authenticity_query: data
                .authenticity_query
                .into_iter()
                .map(Authenticity::from)
                .collect(),
            condition_query: data
                .condition_query
                .into_iter()
                .map(Condition::from)
                .collect(),
            provenance_query: data
                .provenance_query
                .into_iter()
                .map(Provenance::from)
                .collect(),
            restoration_query: data
                .restoration_query
                .into_iter()
                .map(Restoration::from)
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
    use crate::core::product_search::faker::fake_range_query_datetime;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductSearchData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductSearchData {
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                product_query: config.fake_with_rng(rng),
                category_id: config.fake_with_rng(rng),
                period_id: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                exclude_shop_name_query: config.fake_with_rng(rng),
                seller_name_query: config.fake_with_rng(rng),
                exclude_seller_name_query: config.fake_with_rng(rng),
                shop_slug_id_query: config.fake_with_rng(rng),
                exclude_shop_slug_id_query: config.fake_with_rng(rng),
                seller_slug_id_query: config.fake_with_rng(rng),
                exclude_seller_slug_id_query: config.fake_with_rng(rng),
                shop_type_query: config.fake_with_rng(rng),
                country_query: Default::default(),
                continent_query: config.fake_with_rng(rng),
                geo_address_distance_query: None,
                price_query: config
                    .fake_with_rng::<Option<RangeQuery<u32>>, R>(rng) // otherwise get Out-Of-Range-Err often from OpenSearch
                    .map(|query| query.map(u64::from)),
                state_query: config.fake_with_rng(rng),
                origin_year_query: config.fake_with_rng(rng),
                authenticity_query: config.fake_with_rng(rng),
                condition_query: config.fake_with_rng(rng),
                provenance_query: config.fake_with_rng(rng),
                restoration_query: config.fake_with_rng(rng),
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
    use crate::data::authenticity_data::AuthenticityData;
    use crate::data::condition_data::ConditionData;
    use crate::data::product_search_data::ProductSearchData;
    use crate::data::product_state_data::ProductStateData;
    use crate::data::provenance_data::ProvenanceData;
    use crate::data::restoration_data::RestorationData;
    use common::category_key::CategoryId;
    use common::period_key::PeriodId;
    use common::query::range_query::RangeQuery;
    use common::{currency::data::CurrencyData, language::data::LanguageData};
    use serde_json::json;
    use shop::data::shop_type_data::ShopTypeData;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn should_serialize_full() {
        let search_filter = ProductSearchData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            product_query: Some("Boop".try_into().unwrap()),
            category_id: HashSet::from_iter([CategoryId::from("furniture")]),
            period_id: HashSet::from_iter([PeriodId::from("baroque")]),
            shop_name_query: ["Baap".into()].into(),
            exclude_shop_name_query: ["Meow".into()].into(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: HashSet::from_iter([ShopTypeData::CommercialDealer]),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: Some(RangeQuery {
                min: Some(37),
                max: Some(42),
            }),
            state_query: HashSet::from_iter([ProductStateData::Available]),
            origin_year_query: Some(RangeQuery {
                min: Some(1742.into()),
                max: Some(1953.into()),
            }),
            authenticity_query: HashSet::from_iter([AuthenticityData::Original]),
            condition_query: HashSet::from_iter([ConditionData::Excellent]),
            provenance_query: HashSet::from_iter([ProvenanceData::Partial]),
            restoration_query: HashSet::from_iter([RestorationData::Unknown]),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            auction_start_query: None,
            auction_end_query: None,
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
        };
        let expected = json!({
            "language": "de",
            "currency": "EUR",
            "productQuery": "Boop",
            "categoryId": ["furniture"],
            "periodId": ["baroque"],
            "shopName": ["Baap"],
            "excludeShopName": ["Meow"],
            "shopType": ["COMMERCIAL_DEALER"],
            "price": {
                "min": 37,
                "max": 42
            },
            "state": ["AVAILABLE"],
            "originYear": {
                "min": 1742,
                "max": 1953
            },
            "authenticity": ["ORIGINAL"],
            "condition": ["EXCELLENT"],
            "provenance": ["PARTIAL"],
            "restoration": ["UNKNOWN"],
            "created": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z",
            },
            "updated": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z",
            }
        });

        let actual = serde_json::to_value(search_filter).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_full() {
        let json = json!({
            "language": "de",
            "currency": "EUR",
            "productQuery": "Boop",
            "categoryId": ["furniture"],
            "periodId": ["baroque"],
            "shopName": ["Baap"],
            "excludeShopName": ["Meow"],
            "shopType": ["COMMERCIAL_DEALER"],
            "price": {
                "min": 37,
                "max": 42
            },
            "state": ["AVAILABLE"],
            "originYear": {
                "min": 1742,
                "max": 1953
            },
            "authenticity": ["ORIGINAL"],
            "condition": ["EXCELLENT"],
            "provenance": ["PARTIAL"],
            "restoration": ["UNKNOWN"],
            "created": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z",
            },
            "updated": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z",
            }
        });
        let expected = ProductSearchData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            product_query: Some("Boop".try_into().unwrap()),
            category_id: HashSet::from_iter([CategoryId::from("furniture")]),
            period_id: HashSet::from_iter([PeriodId::from("baroque")]),
            shop_name_query: ["Baap".into()].into(),
            exclude_shop_name_query: ["Meow".into()].into(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: HashSet::from_iter([ShopTypeData::CommercialDealer]),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: Some(RangeQuery {
                min: Some(37),
                max: Some(42),
            }),
            state_query: HashSet::from_iter([ProductStateData::Available]),
            origin_year_query: Some(RangeQuery {
                min: Some(1742.into()),
                max: Some(1953.into()),
            }),
            authenticity_query: HashSet::from_iter([AuthenticityData::Original]),
            condition_query: HashSet::from_iter([ConditionData::Excellent]),
            provenance_query: HashSet::from_iter([ProvenanceData::Partial]),
            restoration_query: HashSet::from_iter([RestorationData::Unknown]),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            auction_start_query: None,
            auction_end_query: None,
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
        };

        let actual: ProductSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_multiple_category_and_period_ids() {
        let json = json!({
            "language": "de",
            "currency": "EUR",
            "productQuery": "Boop",
            "categoryId": ["furniture", "decorative-objects"],
            "periodId": ["baroque", "renaissance"],
        });

        let actual: ProductSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(
            HashSet::from_iter([
                CategoryId::from("furniture"),
                CategoryId::from("decorative-objects")
            ]),
            actual.category_id
        );
        assert_eq!(
            HashSet::from_iter([PeriodId::from("baroque"), PeriodId::from("renaissance")]),
            actual.period_id
        );
    }

    #[test]
    fn should_serialize_minimal() {
        let search_filter = ProductSearchData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            product_query: Some("Boop".try_into().unwrap()),
            category_id: Default::default(),
            period_id: Default::default(),
            shop_name_query: Default::default(),
            exclude_shop_name_query: Default::default(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: Default::default(),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: None,
            state_query: Default::default(),
            origin_year_query: None,
            authenticity_query: Default::default(),
            condition_query: Default::default(),
            provenance_query: Default::default(),
            restoration_query: Default::default(),
            created_query: None,
            updated_query: None,
            auction_start_query: None,
            auction_end_query: None,
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
        };
        let expected = json!({
            "language": "de",
            "currency": "EUR",
            "productQuery": "Boop",
        });

        let actual = serde_json::to_value(search_filter).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_minimal() {
        let json = json!({
            "language": "de",
            "currency": "EUR",
            "productQuery": "Boop",
        });
        let expected = ProductSearchData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            product_query: Some("Boop".try_into().unwrap()),
            category_id: Default::default(),
            period_id: Default::default(),
            shop_name_query: Default::default(),
            exclude_shop_name_query: Default::default(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: Default::default(),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: None,
            state_query: Default::default(),
            origin_year_query: None,
            authenticity_query: Default::default(),
            condition_query: Default::default(),
            provenance_query: Default::default(),
            restoration_query: Default::default(),
            created_query: None,
            updated_query: None,
            auction_start_query: None,
            auction_end_query: None,
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
        };

        let actual: ProductSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_minimal_with_default_language_and_currency() {
        let json = json!({
            "productQuery": "Boop",
        });
        let expected = ProductSearchData {
            language: LanguageData::En,
            currency: CurrencyData::Eur,
            product_query: Some("Boop".try_into().unwrap()),
            category_id: Default::default(),
            period_id: Default::default(),
            shop_name_query: Default::default(),
            exclude_shop_name_query: Default::default(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: Default::default(),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: None,
            state_query: Default::default(),
            origin_year_query: None,
            authenticity_query: Default::default(),
            condition_query: Default::default(),
            provenance_query: Default::default(),
            restoration_query: Default::default(),
            created_query: None,
            updated_query: None,
            auction_start_query: None,
            auction_end_query: None,
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
        };

        let actual: ProductSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
