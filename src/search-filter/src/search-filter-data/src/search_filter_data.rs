use std::collections::HashSet;

use item_data::item_state_data::ItemStateData;
use search_filter_core::{range_query::RangeQuery, text_query::TextQuery};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFilterData {
    #[serde(rename = "itemQuery")]
    pub item_query: TextQuery,

    #[serde(
        rename = "shopNameQuery",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub shop_name_query: Option<TextQuery>,

    #[serde(rename = "price", skip_serializing_if = "Option::is_none", default)]
    pub price_query: Option<RangeQuery<u64>>,

    #[serde(rename = "state", skip_serializing_if = "HashSet::is_empty", default)]
    pub state_query: HashSet<ItemStateData>,

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

#[cfg(test)]
mod tests {
    use crate::search_filter_data::SearchFilterData;
    use item_data::item_state_data::ItemStateData;
    use search_filter_core::range_query::RangeQuery;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn should_serialize_full() {
        let search_filter = SearchFilterData {
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
        };
        let expected = json!({
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

        let actual = serde_json::to_value(search_filter).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_full() {
        let json = json!({
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
        let expected = SearchFilterData {
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
        };

        let actual: SearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_serialize_minimal() {
        let search_filter = SearchFilterData {
            item_query: "Boop".try_into().unwrap(),
            shop_name_query: None,
            price_query: None,
            state_query: Default::default(),
            created_query: None,
            updated_query: None,
        };
        let expected = json!({
           "itemQuery": "Boop",
        });

        let actual = serde_json::to_value(search_filter).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_minimal() {
        let json = json!({
           "itemQuery": "Boop",
        });
        let expected = SearchFilterData {
            item_query: "Boop".try_into().unwrap(),
            shop_name_query: None,
            price_query: None,
            state_query: Default::default(),
            created_query: None,
            updated_query: None,
        };

        let actual: SearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
