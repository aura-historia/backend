use product::data::product_search_data::ProductSearchData;
use search_filter::core::user_search_filter_name::UserSearchFilterName;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostUserSearchFilterData {
    pub name: UserSearchFilterName,
    pub search: ProductSearchData,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PostUserSearchFilterData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PostUserSearchFilterData {
                name: config.fake_with_rng(rng),
                search: config.fake_with_rng(rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::post_types::PostUserSearchFilterData;
    use common::distance::data::GeoDistanceQueryData;
    use common::distance::data::{DistanceData, DistanceUnitData};
    use common::query::range_query::RangeQuery;
    use common::shop_name::ShopName;
    use common::shop_slug_id::ShopSlugId;
    use common::{currency::data::CurrencyData, language::data::LanguageData};
    use geo::data::continent_data::ContinentData;
    use product::data::product_search_data::ProductSearchData;
    use product::data::product_state_data::ProductStateData;
    use serde_json::json;
    use shop::data::shop_type_data::ShopTypeData;
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
                "enhancedSearchDescription": "a filter for peppino",
                "shopName": ["Baap"],
                "excludeShopName": ["baddlebap"],
                "shopSlugId": ["imperial-antiques"],
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
                "shopType": ["COMMERCIAL_DEALER"],
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
            }
        });
        let expected = PostUserSearchFilterData {
            name: "hugos filter for peppino".into(),
            search: ProductSearchData {
                language: LanguageData::De,
                currency: CurrencyData::Eur,
                product_query: Some("Boop".try_into().unwrap()),
                enhanced_search_description: Some("a filter for peppino".into()),
                shop_name_query: [ShopName::from("Baap")].into(),
                exclude_shop_name_query: [ShopName::from("baddlebap")].into(),
                seller_name_query: Default::default(),
                exclude_seller_name_query: Default::default(),
                shop_slug_id_query: HashSet::from_iter([ShopSlugId::from("imperial-antiques")]),
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
        };

        let actual: PostUserSearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
