use crate::core::{
    user_search_filter::UserSearchFilter, user_search_filter_id::UserSearchFilterId,
    user_search_filter_name::UserSearchFilterName,
};
use common::user_id::UserId;
use product::data::product_search_data::ProductSearchData;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSearchFilterData {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub name: UserSearchFilterName,

    pub notifications: bool,

    pub search: ProductSearchData,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<UserSearchFilter> for UserSearchFilterData {
    fn from(user_search_filter: UserSearchFilter) -> Self {
        UserSearchFilterData {
            user_id: user_search_filter.user_id,
            user_search_filter_id: user_search_filter.user_search_filter_id,
            name: user_search_filter.name,
            notifications: user_search_filter.notifications,
            search: user_search_filter.search.into(),
            created: user_search_filter.created,
            updated: user_search_filter.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for UserSearchFilterData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UserSearchFilterData {
                user_id: config.fake_with_rng(rng),
                user_search_filter_id: config.fake_with_rng(rng),
                name: config.fake_with_rng(rng),
                notifications: true,
                search: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::user_search_filter_id::UserSearchFilterId;
    use crate::data::user_search_filter_data::UserSearchFilterData;
    use common::category_key::CategoryId;
    use common::period_key::PeriodId;
    use common::query::range_query::RangeQuery;
    use common::{currency::data::CurrencyData, language::data::LanguageData, user_id::UserId};
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
    fn should_serialize() {
        let user_id = UserId::new();
        let search_filter_id = UserSearchFilterId::new();
        let user_search_filter = UserSearchFilterData {
            user_id,
            user_search_filter_id: search_filter_id,
            name: "My Boop Filter".into(),
            notifications: true,
            search: ProductSearchData {
                language: LanguageData::De,
                currency: CurrencyData::Eur,
                product_query: Some("Boop".try_into().unwrap()),
                category_id: HashSet::from_iter([CategoryId::from("furniture")]),
                period_id: HashSet::from_iter([PeriodId::from("baroque")]),
                shop_name_query: ["Baap".into()].into(),
                exclude_shop_name_query: ["baddlebap".into()].into(),
                seller_name_query: Default::default(),
                exclude_seller_name_query: Default::default(),
                shop_type_query: HashSet::from_iter([
                    shop::data::shop_type_data::ShopTypeData::CommercialDealer,
                ]),
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
                origin_year_query: Some(RangeQuery {
                    min: Some(1742.into()),
                    max: Some(1953.into()),
                }),
                authenticity_query: HashSet::from_iter([AuthenticityData::Original]),
                condition_query: HashSet::from_iter([ConditionData::Excellent]),
                provenance_query: HashSet::from_iter([ProvenanceData::Partial]),
                restoration_query: HashSet::from_iter([RestorationData::Unknown]),
                auction_start_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
                auction_end_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
            },
            created: datetime!(2025 - 01 - 01 0:00 UTC),
            updated: datetime!(2025 - 01 - 01 0:00 UTC),
        };
        let expected = json!({
            "userId": user_id.to_string(),
            "userSearchFilterId": search_filter_id.to_string(),
            "name": "My Boop Filter",
            "notifications": true,
            "search": {
                "language": "de",
                "currency": "EUR",
                "productQuery": "Boop",
                "categoryId": ["furniture"],
                "periodId": ["baroque"],
                "shopName": ["Baap"],
                "excludeShopName": ["baddlebap"],
                "shopType": ["COMMERCIAL_DEALER"],
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
                "originYear": {
                    "min": 1742,
                    "max": 1953
                },
                "authenticity": ["ORIGINAL"],
                "condition": ["EXCELLENT"],
                "provenance": ["PARTIAL"],
                "restoration": ["UNKNOWN"],
                "auctionStart": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z",
                },
                "auctionEnd": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z",
                }
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
        let search_filter_id = UserSearchFilterId::new();
        let json = json!({
            "userId": user_id.to_string(),
            "userSearchFilterId": search_filter_id.to_string(),
            "name": "My Boop Filter",
            "notifications": true,
            "search": {
                "language": "de",
                "currency": "EUR",
                "productQuery": "Boop",
                "categoryId": ["furniture"],
                "periodId": ["baroque"],
                "shopName": ["Baap"],
                "excludeShopName": ["baddlebap"],
                "shopType": ["COMMERCIAL_DEALER"],
                "price": {
                "shopType": ["COMMERCIAL_DEALER"],
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
                "originYear": {
                    "min": 1742,
                    "max": 1953
                },
                "authenticity": ["ORIGINAL"],
                "condition": ["EXCELLENT"],
                "provenance": ["PARTIAL"],
                "restoration": ["UNKNOWN"],
                "auctionStart": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z",
                },
                "auctionEnd": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z",
                }
            },
            "created": "2025-01-01T00:00:00Z",
            "updated": "2025-01-01T00:00:00Z"
        });
        let expected = UserSearchFilterData {
            user_id,
            user_search_filter_id: search_filter_id,
            name: "My Boop Filter".into(),
            notifications: true,
            search: ProductSearchData {
                language: LanguageData::De,
                currency: CurrencyData::Eur,
                product_query: Some("Boop".try_into().unwrap()),
                category_id: HashSet::from_iter([CategoryId::from("furniture")]),
                period_id: HashSet::from_iter([PeriodId::from("baroque")]),
                shop_name_query: ["Baap".into()].into(),
                exclude_shop_name_query: ["baddlebap".into()].into(),
                seller_name_query: Default::default(),
                exclude_seller_name_query: Default::default(),
                shop_type_query: HashSet::from_iter([
                    shop::data::shop_type_data::ShopTypeData::CommercialDealer,
                ]),
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
                origin_year_query: Some(RangeQuery {
                    min: Some(1742.into()),
                    max: Some(1953.into()),
                }),
                authenticity_query: HashSet::from_iter([AuthenticityData::Original]),
                condition_query: HashSet::from_iter([ConditionData::Excellent]),
                provenance_query: HashSet::from_iter([ProvenanceData::Partial]),
                restoration_query: HashSet::from_iter([RestorationData::Unknown]),
                auction_start_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
                auction_end_query: Some(RangeQuery {
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
