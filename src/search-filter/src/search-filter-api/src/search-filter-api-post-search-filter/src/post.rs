use search_filter_core::search_filter_name::SearchFilterName;
use search_filter_data::search_filter_data::SearchFilterData;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostUserSearchFilterData {
    pub search_filter_name: SearchFilterName,
    pub search_filter: SearchFilterData,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for PostUserSearchFilterData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PostUserSearchFilterData {
                search_filter_name: config.fake_with_rng(rng),
                search_filter: config.fake_with_rng(rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use common::query::range_query::RangeQuery;
    use common::{currency::data::CurrencyData, language::data::LanguageData};
    use item::data::item_state_data::ItemStateData;
    use search_filter_data::search_filter_data::SearchFilterData;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    use crate::post::PostUserSearchFilterData;

    #[test]
    fn should_deserialize_post_user_search_filter() {
        let json = json!({
            "searchFilterName": "hugos filter for peppino",
            "searchFilter": {
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
            }
        });
        let expected = PostUserSearchFilterData {
            search_filter_name: "hugos filter for peppino".into(),
            search_filter: SearchFilterData {
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
            },
        };

        let actual: PostUserSearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
