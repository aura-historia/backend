use common::query::{range_query::RangeQuery, text_query::TextQuery};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShopSearchData {
    pub shop_name_query: TextQuery,

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

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ShopSearchData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ShopSearchData {
                shop_name_query: Faker.fake(),
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
        use crate::shop_search_data::ShopSearchData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop_search_data() {
            let _ = Faker.fake::<ShopSearchData>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::shop_search_data::ShopSearchData;
    use common::query::range_query::RangeQuery;
    use serde_json::json;
    use time::macros::datetime;

    #[test]
    fn should_deserialize_full() {
        let json = json!({
            "shopNameQuery": "Baap",
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
            shop_name_query: "Baap".try_into().unwrap(),
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
    fn should_deserialize_minimal() {
        let json = json!({
            "shopNameQuery": "Baap",
        });
        let expected = ShopSearchData {
            shop_name_query: "Baap".try_into().unwrap(),
            created: None,
            updated: None,
        };

        let actual: ShopSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
