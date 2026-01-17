use product::data::product_search_data::ProductSearchData;
use search_filter::core::user_search_filter_name::UserSearchFilterName;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostUserSearchFilterData {
    pub name: UserSearchFilterName,
    pub search: ProductSearchData,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for PostUserSearchFilterData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PostUserSearchFilterData {
                name: config.fake_with_rng(rng),
                search: config.fake_with_rng(rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::post::PostUserSearchFilterData;
    use common::query::range_query::RangeQuery;
    use common::{currency::data::CurrencyData, language::data::LanguageData};
    use product::data::authenticity_data::AuthenticityData;
    use product::data::condition_data::ConditionData;
    use product::data::product_search_data::ProductSearchData;
    use product::data::product_state_data::ProductStateData;
    use product::data::provenance_data::ProvenanceData;
    use product::data::restoration_data::RestorationData;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn should_deserialize_post_user_search_filter() {
        let json = json!({
            "name": "hugos filter for peppino",
            "search": {
                "language": "de",
                "currency": "EUR",
                "productQuery": "Boop",
                "shopName": ["Baap"],
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
                },
                "auctionStart": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z",
                },
                "auctionEnd": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z",
                }
            }
        });
        let expected = PostUserSearchFilterData {
            name: "hugos filter for peppino".into(),
            search: ProductSearchData {
                language: LanguageData::De,
                currency: CurrencyData::Eur,
                product_query: "Boop".try_into().unwrap(),
                shop_name_query: ["Baap".into()].into(),
                shop_type_query: HashSet::new(),
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
                auction_start_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
                auction_end_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
            },
        };

        let actual: PostUserSearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
