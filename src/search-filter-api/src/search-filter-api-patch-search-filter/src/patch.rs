use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::{
    currency::{data::CurrencyData, domain::Currency},
    language::{data::LanguageData, domain::Language},
    price::domain::MonetaryAmount,
    product_state::domain::ProductState,
};
use product::data::product_state_data::ProductStateData;
use search_filter::core::user_search_filter_name::UserSearchFilterName;
use search_filter::service::user_search_filter_update::UserSearchFilterUpdate;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PatchUserSearchFilterData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<UserSearchFilterName>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub search: Option<PatchProductSearchData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PatchProductSearchData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language: Option<LanguageData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub currency: Option<CurrencyData>,

    #[serde(rename = "productQuery", skip_serializing_if = "Option::is_none", default)]
    pub product_query: Option<TextQuery>,

    #[serde(
        rename = "shopNameQuery",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub shop_name_query: Option<TextQuery>,

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
}

impl From<PatchUserSearchFilterData> for UserSearchFilterUpdate {
    fn from(patch: PatchUserSearchFilterData) -> Self {
        UserSearchFilterUpdate {
            name: patch.name,
            language: patch
                .search
                .as_ref()
                .and_then(|sf| sf.language.map(Language::from)),
            currency: patch
                .search
                .as_ref()
                .and_then(|sf| sf.currency.map(Currency::from)),
            product_query: patch.search.as_ref().and_then(|sf| sf.product_query.clone()),
            shop_name_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.shop_name_query.clone()),
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
            updated: OffsetDateTime::now_utc(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};
    use product::core::product_search::faker::fake_range_query_datetime;

    impl Dummy<Faker> for PatchProductSearchData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PatchProductSearchData {
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                product_query: config.fake_with_rng(rng),
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
    use crate::patch::{PatchProductSearchData, PatchUserSearchFilterData};
    use common::query::range_query::RangeQuery;
    use common::{currency::data::CurrencyData, language::data::LanguageData};
    use product::data::product_state_data::ProductStateData;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn should_deserialize_search_filter_patch() {
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
        let expected = PatchProductSearchData {
            language: Some(LanguageData::De),
            currency: Some(CurrencyData::Eur),
            product_query: Some("Boop".try_into().unwrap()),
            shop_name_query: Some("Baap".try_into().unwrap()),
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
            }
        });
        let expected = PatchUserSearchFilterData {
            name: Some("hugos filter for peppino".into()),
            search: Some(PatchProductSearchData {
                language: Some(LanguageData::De),
                currency: Some(CurrencyData::Eur),
                product_query: Some("Boop".try_into().unwrap()),
                shop_name_query: Some("Baap".try_into().unwrap()),
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
            }),
        };

        let actual: PatchUserSearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
