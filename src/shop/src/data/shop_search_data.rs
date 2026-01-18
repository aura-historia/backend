use crate::core::shop_search::ShopSearch;
use crate::core::shop_type::ShopType;
use crate::data::shop_type_data::ShopTypeData;
use common::query::{range_query::RangeQuery, text_query::TextQuery};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShopSearchData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_name_query: Option<TextQuery<0>>,

    #[serde(
        rename = "shopType",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub shop_type_query: HashSet<ShopTypeData>,

    #[serde(
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub created: Option<RangeQuery<OffsetDateTime>>,

    #[serde(
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub updated: Option<RangeQuery<OffsetDateTime>>,
}

impl From<ShopSearch> for ShopSearchData {
    fn from(search: ShopSearch) -> Self {
        ShopSearchData {
            shop_name_query: search.shop_name_query,
            shop_type_query: search
                .shop_type_query
                .into_iter()
                .map(ShopTypeData::from)
                .collect(),
            created: search.created,
            updated: search.updated,
        }
    }
}

impl From<ShopSearchData> for ShopSearch {
    fn from(data: ShopSearchData) -> Self {
        ShopSearch {
            shop_name_query: data.shop_name_query,
            shop_type_query: data
                .shop_type_query
                .into_iter()
                .map(ShopType::from)
                .collect(),
            created: data.created,
            updated: data.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ShopSearchData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ShopSearchData {
                shop_name_query: Faker.fake(),
                shop_type_query: config.fake_with_rng(rng),
                created: fake_range_query_datetime(config, rng),
                updated: fake_range_query_datetime(config, rng),
            }
        }
    }

    pub fn fake_range_query_datetime<R: fake::Rng + ?Sized>(
        config: &Faker,
        rng: &mut R,
    ) -> Option<RangeQuery<OffsetDateTime>> {
        if config.fake_with_rng(rng) {
            None
        } else {
            let min = if config.fake_with_rng(rng) {
                Some(OffsetDateTime::now_utc())
            } else {
                None
            };
            let max = if config.fake_with_rng(rng) {
                Some(OffsetDateTime::now_utc())
            } else {
                None
            };
            Some(RangeQuery { min, max })
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::data::shop_search_data::ShopSearchData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop_search_data() {
            let _ = Faker.fake::<ShopSearchData>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::shop_search_data::ShopSearchData;
    use crate::data::shop_type_data::ShopTypeData;
    use common::query::range_query::RangeQuery;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn should_deserialize_full() {
        let json = json!({
            "shopNameQuery": "Baap",
            "shopType": ["AUCTION_HOUSE"],
            "created": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z",
            },
            "updated": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z",
            }
        });
        let expected = ShopSearchData {
            shop_name_query: Some("Baap".try_into().unwrap()),
            shop_type_query: HashSet::from_iter([ShopTypeData::AuctionHouse]),
            created: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            updated: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
        };

        let actual: ShopSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_when_only_shop_name_query() {
        let json = json!({
            "shopNameQuery": "Baap",
        });
        let expected = ShopSearchData {
            shop_name_query: Some("Baap".try_into().unwrap()),
            shop_type_query: Default::default(),
            created: None,
            updated: None,
        };

        let actual: ShopSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_when_empty() {
        let json = json!({});
        let expected = ShopSearchData {
            shop_name_query: None,
            shop_type_query: Default::default(),
            created: None,
            updated: None,
        };

        let actual: ShopSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
