use crate::search_filter_data::SearchFilterData;
use common::user_id::UserId;
use search_filter_core::{
    search_filter_id::SearchFilterId, search_filter_name::SearchFilterName,
    user_search_filter::UserSearchFilter,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSearchFilterData {
    pub user_id: UserId,
    pub search_filter_id: SearchFilterId,
    pub search_filter_name: SearchFilterName,

    pub search_filter: SearchFilterData,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<UserSearchFilter> for UserSearchFilterData {
    fn from(user_search_filter: UserSearchFilter) -> Self {
        UserSearchFilterData {
            user_id: user_search_filter.user_id,
            search_filter_id: user_search_filter.search_filter_id,
            search_filter_name: user_search_filter.search_filter_name,
            search_filter: user_search_filter.search_filter.into(),
            created: user_search_filter.created,
            updated: user_search_filter.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for UserSearchFilterData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UserSearchFilterData {
                user_id: config.fake_with_rng(rng),
                search_filter_id: config.fake_with_rng(rng),
                search_filter_name: config.fake_with_rng(rng),
                search_filter: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        search_filter_data::SearchFilterData, user_search_filter_data::UserSearchFilterData,
    };
    use common::query::range_query::RangeQuery;
    use common::{currency::data::CurrencyData, language::data::LanguageData, user_id::UserId};
    use item_data::item_state_data::ItemStateData;
    use search_filter_core::search_filter_id::SearchFilterId;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn should_serialize() {
        let user_id = UserId::new();
        let search_filter_id = SearchFilterId::new();
        let user_search_filter = UserSearchFilterData {
            user_id,
            search_filter_id,
            search_filter_name: "My Boop Filter".into(),
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
            created: datetime!(2025 - 01 - 01 0:00 UTC),
            updated: datetime!(2025 - 01 - 01 0:00 UTC),
        };
        let expected = json!({
            "userId": user_id.to_string(),
            "searchFilterId": search_filter_id.to_string(),
            "searchFilterName": "My Boop Filter",
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
                },
            },
            "created": "2025-01-01T00:00:00Z",
            "updated": "2025-01-01T00:00:00Z"
        });

        let actual = serde_json::to_value(user_search_filter).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize() {
        let user_id = UserId::new();
        let search_filter_id = SearchFilterId::new();
        let json = json!({
            "userId": user_id.to_string(),
            "searchFilterId": search_filter_id.to_string(),
            "searchFilterName": "My Boop Filter",
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
                },
            },
            "created": "2025-01-01T00:00:00Z",
            "updated": "2025-01-01T00:00:00Z"
        });
        let expected = UserSearchFilterData {
            user_id,
            search_filter_id,
            search_filter_name: "My Boop Filter".into(),
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
            created: datetime!(2025 - 01 - 01 0:00 UTC),
            updated: datetime!(2025 - 01 - 01 0:00 UTC),
        };

        let actual: UserSearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
