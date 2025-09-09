use std::collections::HashSet;

use common::{
    currency::data::CurrencyData, item_state::domain::ItemState, language::data::LanguageData,
    price::domain::MonetaryAmount,
};
use item_data::item_state_data::ItemStateData;
use search_filter_core::{
    range_query::RangeQuery, search_filter::SearchFilter, text_query::TextQuery,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFilterData {
    pub language: LanguageData,

    pub currency: CurrencyData,

    #[serde(rename = "itemQuery")]
    pub item_query: TextQuery,

    #[serde(
        rename = "shopNameQuery",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub shop_name_query: Option<TextQuery>,

    #[serde(rename = "price", skip_serializing_if = "Option::is_none", default)]
    pub price_query: Option<RangeQuery<u64>>,

    #[serde(rename = "state", skip_serializing_if = "HashSet::is_empty", default)]
    pub state_query: HashSet<ItemStateData>,

    #[serde(
        rename = "created",
        with = "search_filter_core::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub created_query: Option<RangeQuery<OffsetDateTime>>,

    #[serde(
        rename = "updated",
        with = "search_filter_core::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
}

impl From<SearchFilter> for SearchFilterData {
    fn from(search_filter: SearchFilter) -> Self {
        SearchFilterData {
            language: search_filter.language.into(),
            currency: search_filter.currency.into(),
            item_query: search_filter.item_query,
            shop_name_query: search_filter.shop_name_query,
            price_query: search_filter
                .price_query
                .map(|price_query| price_query.map(u64::from)),
            state_query: search_filter
                .state_query
                .into_iter()
                .map(ItemStateData::from)
                .collect(),
            created_query: search_filter.created_query,
            updated_query: search_filter.updated_query,
        }
    }
}

impl From<SearchFilterData> for SearchFilter {
    fn from(data: SearchFilterData) -> Self {
        SearchFilter {
            language: data.language.into(),
            currency: data.currency.into(),
            item_query: data.item_query,
            shop_name_query: data.shop_name_query,
            price_query: data
                .price_query
                .map(|query| query.map(MonetaryAmount::from)),
            state_query: data.state_query.into_iter().map(ItemState::from).collect(),
            created_query: data.created_query,
            updated_query: data.updated_query,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};
    use search_filter_core::search_filter::faker::fake_range_query_datetime;

    impl Dummy<Faker> for SearchFilterData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            SearchFilterData {
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                item_query: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::search_filter_data::SearchFilterData;
    use common::{currency::data::CurrencyData, language::data::LanguageData};
    use item_data::item_state_data::ItemStateData;
    use search_filter_core::range_query::RangeQuery;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn should_serialize_full() {
        let search_filter = SearchFilterData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            item_query: "Boop".try_into().unwrap(),
            shop_name_query: Some("Baap".try_into().unwrap()),
            price_query: Some(RangeQuery {
                min: Some(37),
                max: Some(42),
            }),
            state_query: HashSet::from_iter([ItemStateData::Available]),
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
            "itemQuery": "Boop",
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
            "itemQuery": "Boop",
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
        let expected = SearchFilterData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            item_query: "Boop".try_into().unwrap(),
            shop_name_query: Some("Baap".try_into().unwrap()),
            price_query: Some(RangeQuery {
                min: Some(37),
                max: Some(42),
            }),
            state_query: HashSet::from_iter([ItemStateData::Available]),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
        };

        let actual: SearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_serialize_minimal() {
        let search_filter = SearchFilterData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            item_query: "Boop".try_into().unwrap(),
            shop_name_query: None,
            price_query: None,
            state_query: Default::default(),
            created_query: None,
            updated_query: None,
        };
        let expected = json!({
            "language": "de",
            "currency": "EUR",
            "itemQuery": "Boop",
        });

        let actual = serde_json::to_value(search_filter).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_minimal() {
        let json = json!({
            "language": "de",
            "currency": "EUR",
            "itemQuery": "Boop",
        });
        let expected = SearchFilterData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            item_query: "Boop".try_into().unwrap(),
            shop_name_query: None,
            price_query: None,
            state_query: Default::default(),
            created_query: None,
            updated_query: None,
        };

        let actual: SearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
