use crate::core::product_search::ProductSearch;
use crate::data::product_state_data::ProductStateData;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::{
    currency::data::CurrencyData, language::data::LanguageData, price::domain::MonetaryAmount,
    product_state::domain::ProductState,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductSearchData {
    pub language: LanguageData,

    pub currency: CurrencyData,

    #[serde(rename = "productQuery")]
    pub product_query: TextQuery,

    #[serde(
        rename = "shopNameQuery",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub shop_name_query: Option<TextQuery>,

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
}

impl From<ProductSearch> for ProductSearchData {
    fn from(search_filter: ProductSearch) -> Self {
        ProductSearchData {
            language: search_filter.language.into(),
            currency: search_filter.currency.into(),
            product_query: search_filter.product_query,
            shop_name_query: search_filter.shop_name_query,
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
        }
    }
}

impl From<ProductSearchData> for ProductSearch {
    fn from(data: ProductSearchData) -> Self {
        ProductSearch {
            language: data.language.into(),
            currency: data.currency.into(),
            product_query: data.product_query,
            shop_name_query: data.shop_name_query,
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
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::product_search::faker::fake_range_query_datetime;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ProductSearchData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductSearchData {
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                product_query: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                price_query: config
                    .fake_with_rng::<Option<RangeQuery<u32>>, R>(rng) // otherwise get Out-Of-Range-Err often from OpenSearch
                    .map(|query| query.map(u64::from)),
                state_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::product_search_data::ProductSearchData;
    use crate::data::product_state_data::ProductStateData;
    use common::query::range_query::RangeQuery;
    use common::{currency::data::CurrencyData, language::data::LanguageData};
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn should_serialize_full() {
        let search_filter = ProductSearchData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            product_query: "Boop".try_into().unwrap(),
            shop_name_query: Some("Baap".try_into().unwrap()),
            price_query: Some(RangeQuery {
                min: Some(37),
                max: Some(42),
            }),
            state_query: HashSet::from_iter([ProductStateData::Available]),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
        };
        let expected = json!({
            "language": "de",
            "currency": "EUR",
            "productQuery": "Boop",
            "shopNameQuery": "Baap",
            "price": {
                "min": 37,
                "max": 42
            },
            "state": ["AVAILABLE"],
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
            "shopNameQuery": "Baap",
            "price": {
                "min": 37,
                "max": 42
            },
            "state": ["AVAILABLE"],
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
            product_query: "Boop".try_into().unwrap(),
            shop_name_query: Some("Baap".try_into().unwrap()),
            price_query: Some(RangeQuery {
                min: Some(37),
                max: Some(42),
            }),
            state_query: HashSet::from_iter([ProductStateData::Available]),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
        };

        let actual: ProductSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_serialize_minimal() {
        let search_filter = ProductSearchData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            product_query: "Boop".try_into().unwrap(),
            shop_name_query: None,
            price_query: None,
            state_query: Default::default(),
            created_query: None,
            updated_query: None,
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
            product_query: "Boop".try_into().unwrap(),
            shop_name_query: None,
            price_query: None,
            state_query: Default::default(),
            created_query: None,
            updated_query: None,
        };

        let actual: ProductSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
