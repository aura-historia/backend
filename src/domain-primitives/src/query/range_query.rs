use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub struct RangeQuery<T> {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub min: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max: Option<T>,
}

impl<T: Eq> Eq for RangeQuery<T> {}

impl<T: PartialOrd> PartialOrd for RangeQuery<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.min.partial_cmp(&other.min) {
            Some(std::cmp::Ordering::Equal) => self.max.partial_cmp(&other.max),
            ordering => ordering,
        }
    }
}

impl<T: Ord> Ord for RangeQuery<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.min
            .cmp(&other.min)
            .then_with(|| self.max.cmp(&other.max))
    }
}

impl<T> Default for RangeQuery<T> {
    fn default() -> Self {
        Self {
            min: None,
            max: None,
        }
    }
}

impl<T> RangeQuery<T> {
    pub fn map<U, F>(self, mut f: F) -> RangeQuery<U>
    where
        F: FnMut(T) -> U,
    {
        RangeQuery {
            min: self.min.map(&mut f),
            max: self.max.map(f),
        }
    }
}

/// A serde adapter for `RangeQuery<OffsetDateTime>` using RFC3339.
pub mod range_rfc3339 {
    use super::RangeQuery;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use time::OffsetDateTime;

    pub fn serialize<S>(
        range: &RangeQuery<OffsetDateTime>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Helper {
            #[serde(
                with = "time::serde::rfc3339::option",
                skip_serializing_if = "Option::is_none"
            )]
            min: Option<OffsetDateTime>,
            #[serde(
                with = "time::serde::rfc3339::option",
                skip_serializing_if = "Option::is_none"
            )]
            max: Option<OffsetDateTime>,
        }

        let helper = Helper {
            min: range.min,
            max: range.max,
        };
        helper.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<RangeQuery<OffsetDateTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            #[serde(with = "time::serde::rfc3339::option", default)]
            min: Option<OffsetDateTime>,
            #[serde(with = "time::serde::rfc3339::option", default)]
            max: Option<OffsetDateTime>,
        }

        let helper = Helper::deserialize(deserializer)?;
        Ok(RangeQuery {
            min: helper.min,
            max: helper.max,
        })
    }

    /// Support for `Option<RangeQuery<OffsetDateTime>>`
    pub mod option {
        use super::*;
        use serde::{Deserializer, Serializer};

        pub fn serialize<S>(
            value: &Option<RangeQuery<OffsetDateTime>>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match value {
                Some(v) => super::serialize(v, serializer),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<Option<RangeQuery<OffsetDateTime>>, D::Error>
        where
            D: Deserializer<'de>,
        {
            Ok(Some(super::deserialize(deserializer)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest;

    use super::*;
    use serde_json::{self, Value, json};
    use time::{OffsetDateTime, macros::datetime};

    // Helpers for serde testing
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Wrapper {
        #[serde(with = "super::range_rfc3339")]
        range: RangeQuery<OffsetDateTime>,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct WrapperOpt {
        #[serde(
            with = "super::range_rfc3339::option",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        range: Option<RangeQuery<OffsetDateTime>>,
    }

    #[test]
    fn should_map_values_when_both_min_and_max_present() {
        let rq = RangeQuery {
            min: Some(1),
            max: Some(10),
        };
        let mapped = rq.map(|x| x.to_string());

        assert_eq!(
            mapped,
            RangeQuery {
                min: Some("1".into()),
                max: Some("10".into())
            }
        );
    }

    #[test]
    fn should_map_values_when_only_min_present() {
        let rq = RangeQuery {
            min: Some(42),
            max: None,
        };
        let mapped = rq.map(|x| x * 2);

        assert_eq!(
            mapped,
            RangeQuery {
                min: Some(84),
                max: None
            }
        );
    }

    #[test]
    fn should_map_values_when_empty_rangequery() {
        let rq: RangeQuery<i32> = RangeQuery {
            min: None,
            max: None,
        };
        let mapped = rq.map(|x| x * 2);

        assert_eq!(
            mapped,
            RangeQuery {
                min: None,
                max: None
            }
        );
    }

    #[rstest::rstest]
    #[case(RangeQuery { min: None, max: None }, json!({"range": {}}))]
    #[case(RangeQuery { min: Some(datetime!(2021 - 01 - 01 0:00 UTC)), max: None }, json!({"range": { "min":"2021-01-01T00:00:00Z" }}))]
    #[case(RangeQuery { min: None, max: Some(datetime!(2021 - 01 - 01 0:00 UTC)) }, json!({"range": { "max":"2021-01-01T00:00:00Z" }}))]
    #[case(RangeQuery { min: Some(datetime!(2021 - 01 - 01 0:00 UTC)), max: Some(datetime!(2021 - 01 - 01 0:00 UTC)) }, json!({"range": { "min":"2021-01-01T00:00:00Z", "max":"2021-01-01T00:00:00Z" }}))]
    #[trace]
    fn should_serialize_rangequery_for_rfc3339(
        #[case] range: RangeQuery<OffsetDateTime>,
        #[case] expected: Value,
    ) {
        let wrapper = Wrapper { range };

        let actual = serde_json::to_value(&wrapper).unwrap();
        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case(RangeQuery { min: None, max: None }, json!({"range": {}}))]
    #[case(RangeQuery { min: None, max: None }, json!({"range": {"min": null}}))]
    #[case(RangeQuery { min: None, max: None }, json!({"range": {"max": null}}))]
    #[case(RangeQuery { min: None, max: None }, json!({"range": {"min": null, "max": null}}))]
    #[case(RangeQuery { min: Some(datetime!(2021 - 01 - 01 0:00 UTC)), max: None }, json!({"range": { "min":"2021-01-01T00:00:00Z" }}))]
    #[case(RangeQuery { min: Some(datetime!(2021 - 01 - 01 0:00 UTC)), max: None }, json!({"range": { "min":"2021-01-01T00:00:00Z", "max": null }}))]
    #[case(RangeQuery { min: None, max: Some(datetime!(2021 - 01 - 01 0:00 UTC)) }, json!({"range": { "max":"2021-01-01T00:00:00Z" }}))]
    #[case(RangeQuery { min: None, max: Some(datetime!(2021 - 01 - 01 0:00 UTC)) }, json!({"range": { "min": null, "max":"2021-01-01T00:00:00Z" }}))]
    #[case(RangeQuery { min: Some(datetime!(2021 - 01 - 01 0:00 UTC)), max: Some(datetime!(2021 - 01 - 01 0:00 UTC)) }, json!({"range": { "min":"2021-01-01T00:00:00Z", "max":"2021-01-01T00:00:00Z" }}))]
    #[trace]
    fn should_deserialize_rangequery_for_rfc3339(
        #[case] expected: RangeQuery<OffsetDateTime>,
        #[case] json: Value,
    ) {
        let expected = Wrapper { range: expected };

        let actual: Wrapper = serde_json::from_value(json).unwrap();
        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case(RangeQuery { min: None, max: None }, json!({"range": {}}))]
    #[case(RangeQuery { min: Some(datetime!(2021 - 01 - 01 0:00 UTC)), max: None }, json!({"range": { "min":"2021-01-01T00:00:00Z" }}))]
    #[case(RangeQuery { min: None, max: Some(datetime!(2021 - 01 - 01 0:00 UTC)) }, json!({"range": { "max":"2021-01-01T00:00:00Z" }}))]
    #[case(RangeQuery { min: Some(datetime!(2021 - 01 - 01 0:00 UTC)), max: Some(datetime!(2021 - 01 - 01 0:00 UTC)) }, json!({"range": { "min":"2021-01-01T00:00:00Z", "max":"2021-01-01T00:00:00Z" }}))]
    #[trace]
    fn should_serialize_rangequery_for_rfc3339_option(
        #[case] range: RangeQuery<OffsetDateTime>,
        #[case] expected: Value,
    ) {
        let wrapper = WrapperOpt { range: Some(range) };

        let actual = serde_json::to_value(&wrapper).unwrap();
        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case(RangeQuery { min: None, max: None }, json!({"range": {}}))]
    #[case(RangeQuery { min: None, max: None }, json!({"range": {"min": null}}))]
    #[case(RangeQuery { min: None, max: None }, json!({"range": {"max": null}}))]
    #[case(RangeQuery { min: None, max: None }, json!({"range": {"min": null, "max": null}}))]
    #[case(RangeQuery { min: Some(datetime!(2021 - 01 - 01 0:00 UTC)), max: None }, json!({"range": { "min":"2021-01-01T00:00:00Z" }}))]
    #[case(RangeQuery { min: Some(datetime!(2021 - 01 - 01 0:00 UTC)), max: None }, json!({"range": { "min":"2021-01-01T00:00:00Z", "max": null }}))]
    #[case(RangeQuery { min: None, max: Some(datetime!(2021 - 01 - 01 0:00 UTC)) }, json!({"range": { "max":"2021-01-01T00:00:00Z" }}))]
    #[case(RangeQuery { min: None, max: Some(datetime!(2021 - 01 - 01 0:00 UTC)) }, json!({"range": { "min": null, "max":"2021-01-01T00:00:00Z" }}))]
    #[case(RangeQuery { min: Some(datetime!(2021 - 01 - 01 0:00 UTC)), max: Some(datetime!(2021 - 01 - 01 0:00 UTC)) }, json!({"range": { "min":"2021-01-01T00:00:00Z", "max":"2021-01-01T00:00:00Z" }}))]
    #[trace]
    fn should_deserialize_rangequery_for_rfc3339_option(
        #[case] expected: RangeQuery<OffsetDateTime>,
        #[case] json: Value,
    ) {
        let expected = WrapperOpt {
            range: Some(expected),
        };

        let actual: WrapperOpt = serde_json::from_value(json).unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn should_serialize_rangequery_when_none_for_rfc3339_option() {
        let wrapper = WrapperOpt { range: None };
        let json = serde_json::to_string(&wrapper).unwrap();
        assert_eq!(json, r#"{}"#);
    }

    #[test]
    fn should_deserialize_rangequery_as_none_when_empty_for_rfc3339_option() {
        let json = json!({});
        let wrapper: WrapperOpt = serde_json::from_value(json).unwrap();
        assert_eq!(wrapper.range, None);
    }

    #[test]
    fn should_deserialize_rangequery_as_empty_some_when_empty_for_rfc3339_option() {
        let json = json!({"range": {}});
        let wrapper: WrapperOpt = serde_json::from_value(json).unwrap();
        assert_eq!(
            wrapper.range,
            Some(RangeQuery {
                min: None,
                max: None
            })
        );
    }
}
