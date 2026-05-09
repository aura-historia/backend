use crate::core::{
    user_search_filter::UserSearchFilter, user_search_filter_name::UserSearchFilterName,
};
use common::user_search_filter_id::UserSearchFilterId;
use common::{resource_state::data::ResourceStateData, user_id::UserId};
use product::data::product_search_data::ProductSearchData;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSearchFilterData {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub name: UserSearchFilterName,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enhanced_search_description: Option<String>,

    pub notifications: bool,
    #[serde(default)]
    pub state: ResourceStateData,

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
            enhanced_search_description: user_search_filter
                .enhanced_search_description
                .map(Into::into),
            notifications: user_search_filter.notifications,
            state: user_search_filter.state.into(),
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
                enhanced_search_description: config.fake_with_rng(rng),
                notifications: true,
                state: ResourceStateData::Active,
                search: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::user_search_filter_data::UserSearchFilterData;
    use common::distance::data::GeoDistanceQueryData;
    use common::distance::data::{DistanceData, DistanceUnitData};
    use common::query::range_query::RangeQuery;
    use common::resource_state::data::ResourceStateData;
    use common::shop_name::ShopName;
    use common::slug_id::SlugId;
    use common::user_search_filter_id::UserSearchFilterId;
    use common::{currency::data::CurrencyData, language::data::LanguageData, user_id::UserId};
    use geo::data::continent_data::ContinentData;
    use product::data::product_search_data::ProductSearchData;
    use product::data::product_state_data::ProductStateData;
    use serde_json::json;
    use shop::data::shop_type_data::ShopTypeData;
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
            enhanced_search_description: Some("This is a filter for Boop products".into()),
            notifications: true,
            state: ResourceStateData::Active,
            search: ProductSearchData {
                language: LanguageData::De,
                currency: CurrencyData::Eur,
                product_query: Some("Boop".try_into().unwrap()),
                shop_name_query: ["Baap".into()].into(),
                exclude_shop_name_query: ["baddlebap".into()].into(),
                seller_name_query: Default::default(),
                exclude_seller_name_query: Default::default(),
                shop_slug_id_query: HashSet::from_iter([SlugId::from("imperial-antiques")]),
                exclude_shop_slug_id_query: Default::default(),
                seller_slug_id_query: Default::default(),
                exclude_seller_slug_id_query: Default::default(),
                shop_type_query: HashSet::from_iter([ShopTypeData::CommercialDealer]),
                country_query: HashSet::from_iter([isocountry::CountryCode::DEU]),
                continent_query: HashSet::from_iter([ContinentData::Europe]),
                geo_address_distance_query: Some(GeoDistanceQueryData {
                    lat: 52.52,
                    lon: 13.405,
                    distance: DistanceData {
                        amount: 100.0,
                        unit: DistanceUnitData::Kilometers,
                    },
                }),
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
                auction_start_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
                auction_end_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
            },
            created: datetime!(2000 - 05 - 04 0:00 UTC),
            updated: datetime!(2025 - 05 - 04 0:00 UTC),
        };

        let expected = json!({
            "userId": user_id.to_string(),
            "userSearchFilterId": search_filter_id.to_string(),
            "name": "My Boop Filter",
            "enhancedSearchDescription": "This is a filter for Boop products",
            "notifications": true,
            "state": "ACTIVE",
            "search": {
                "language": "de",
                "currency": "EUR",
                "productQuery": "Boop",
                "shopName": ["Baap"],
                "excludeShopName": ["baddlebap"],
                "shopSlugId": ["imperial-antiques"],
                "shopType": ["COMMERCIAL_DEALER"],
                "country": ["DE"],
                "continent": ["EUROPE"],
                "geoAddress": {
                    "lat": 52.52,
                    "lon": 13.405,
                    "distance": {
                        "amount": 100.0,
                        "unit": "KILOMETERS"
                    }
                },
                "price": {
                    "min": 37,
                    "max": 42
                },
                "state": ["AVAILABLE"],
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
            },
            "created": "2000-05-04T00:00:00Z",
            "updated": "2025-05-04T00:00:00Z"
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
            "enhancedSearchDescription": "This is a filter for Boop products",
            "notifications": true,
            "search": {
                "language": "de",
                "currency": "EUR",
                "productQuery": "Boop",
                "shopName": ["Baap"],
                "excludeShopName": ["baddlebap"],
                "shopSlugId": ["imperial-antiques"],
                "shopType": ["COMMERCIAL_DEALER"],
                "country": ["DE"],
                "continent": ["EUROPE"],
                "geoAddress": {
                    "lat": 52.52,
                    "lon": 13.405,
                    "distance": {
                        "amount": 100.0,
                        "unit": "KILOMETERS"
                    }
                },
                "price": {
                    "min": 37,
                    "max": 42
                },
                "state": ["AVAILABLE"],
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
            },
            "created": "2000-05-04T00:00:00Z",
            "updated": "2025-05-04T00:00:00Z"
        });
        let expected = UserSearchFilterData {
            user_id,
            user_search_filter_id: search_filter_id,
            name: "My Boop Filter".into(),
            enhanced_search_description: Some("This is a filter for Boop products".into()),
            notifications: true,
            state: ResourceStateData::Active,
            search: ProductSearchData {
                language: LanguageData::De,
                currency: CurrencyData::Eur,
                product_query: Some("Boop".try_into().unwrap()),
                shop_name_query: [ShopName::from("Baap")].into(),
                exclude_shop_name_query: [ShopName::from("baddlebap")].into(),
                seller_name_query: Default::default(),
                exclude_seller_name_query: Default::default(),
                shop_slug_id_query: HashSet::from_iter([SlugId::from("imperial-antiques")]),
                exclude_shop_slug_id_query: Default::default(),
                seller_slug_id_query: Default::default(),
                exclude_seller_slug_id_query: Default::default(),
                shop_type_query: HashSet::from_iter([ShopTypeData::CommercialDealer]),
                country_query: HashSet::from_iter([isocountry::CountryCode::DEU]),
                continent_query: HashSet::from_iter([ContinentData::Europe]),
                geo_address_distance_query: Some(GeoDistanceQueryData {
                    lat: 52.52,
                    lon: 13.405,
                    distance: DistanceData {
                        amount: 100.0,
                        unit: DistanceUnitData::Kilometers,
                    },
                }),
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
                auction_start_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
                auction_end_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
            },
            created: datetime!(2000 - 05 - 04 0:00 UTC),
            updated: datetime!(2025 - 05 - 04 0:00 UTC),
        };

        let actual: UserSearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
