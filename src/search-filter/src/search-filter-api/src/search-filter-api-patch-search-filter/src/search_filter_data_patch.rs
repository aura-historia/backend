use common::{
    currency::{data::CurrencyData, domain::Currency},
    item_state::domain::ItemState,
    language::{data::LanguageData, domain::Language},
    price::domain::MonetaryAmount,
};
use item_data::item_state_data::ItemStateData;
use search_filter_core::{range_query::RangeQuery, text_query::TextQuery};
use search_filter_service::search_filter_update::SearchFilterUpdate;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SearchFilterDataPatch {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language: Option<LanguageData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub currency: Option<CurrencyData>,

    #[serde(rename = "itemQuery", skip_serializing_if = "Option::is_none", default)]
    pub item_query: Option<TextQuery>,

    #[serde(
        rename = "shopNameQuery",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub shop_name_query: Option<TextQuery>,

    #[serde(rename = "price", skip_serializing_if = "Option::is_none", default)]
    pub price_query: Option<RangeQuery<u64>>,

    #[serde(rename = "state", skip_serializing_if = "Option::is_none", default)]
    pub state_query: Option<HashSet<ItemStateData>>,

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

impl From<SearchFilterDataPatch> for SearchFilterUpdate {
    fn from(data: SearchFilterDataPatch) -> Self {
        SearchFilterUpdate {
            language: data.language.map(Language::from),
            currency: data.currency.map(Currency::from),
            item_query: data.item_query,
            shop_name_query: data.shop_name_query,
            price_query: data
                .price_query
                .map(|query| query.map(MonetaryAmount::from)),
            state_query: data
                .state_query
                .map(|states| states.into_iter().map(ItemState::from).collect()),
            created_query: data.created_query,
            updated_query: data.updated_query,
            updated: OffsetDateTime::now_utc(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};
    use search_filter_core::search_filter::faker::fake_range_query_datetime;

    impl Dummy<Faker> for SearchFilterDataPatch {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            SearchFilterDataPatch {
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
    use crate::search_filter_data_patch::SearchFilterDataPatch;
    use common::{currency::data::CurrencyData, language::data::LanguageData};
    use item_data::item_state_data::ItemStateData;
    use search_filter_core::range_query::RangeQuery;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn should_deserialize() {
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
        let expected = SearchFilterDataPatch {
            language: Some(LanguageData::De),
            currency: Some(CurrencyData::Eur),
            item_query: Some("Boop".try_into().unwrap()),
            shop_name_query: Some("Baap".try_into().unwrap()),
            price_query: Some(RangeQuery {
                min: Some(37),
                max: Some(42),
            }),
            state_query: Some(HashSet::from_iter([ItemStateData::Available])),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
        };

        let actual: SearchFilterDataPatch = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
