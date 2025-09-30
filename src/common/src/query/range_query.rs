use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Serialize, Deserialize)]
pub struct RangeQuery<T: Ord> {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub min: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max: Option<T>,
}

impl<T: Ord> Default for RangeQuery<T> {
    fn default() -> Self {
        Self {
            min: None,
            max: None,
        }
    }
}

impl<T: Ord> RangeQuery<T> {
    pub fn map<U, F>(self, mut f: F) -> RangeQuery<U>
    where
        U: Ord,
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
            #[serde(with = "time::serde::rfc3339::option")]
            min: Option<OffsetDateTime>,
            #[serde(with = "time::serde::rfc3339::option")]
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
            #[serde(with = "time::serde::rfc3339::option")]
            min: Option<OffsetDateTime>,
            #[serde(with = "time::serde::rfc3339::option")]
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
    use super::*;
    use serde_json;
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

    // --- map tests ---

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

    // --- serde tests for RangeQuery<OffsetDateTime> ---

    #[test]
    fn should_serialize_rangequery_when_both_min_and_max_present_for_rfc3339() {
        let wrapper = Wrapper {
            range: RangeQuery {
                min: Some(datetime!(2021-01-01 00:00 UTC)),
                max: Some(datetime!(2021-12-31 23:59:59 UTC)),
            },
        };

        let json = serde_json::to_string(&wrapper).unwrap();
        assert_eq!(
            json,
            r#"{"range":{"min":"2021-01-01T00:00:00Z","max":"2021-12-31T23:59:59Z"}}"#
        );
    }

    #[test]
    fn should_deserialize_rangequery_when_both_min_and_max_present_for_rfc3339() {
        let json = r#"{"range":{"min":"2021-01-01T00:00:00Z","max":"2021-12-31T23:59:59Z"}}"#;

        let wrapper: Wrapper = serde_json::from_str(json).unwrap();

        assert_eq!(
            wrapper,
            Wrapper {
                range: RangeQuery {
                    min: Some(datetime!(2021-01-01 00:00 UTC)),
                    max: Some(datetime!(2021-12-31 23:59:59 UTC))
                }
            }
        );
    }

    #[test]
    fn should_serialize_rangequery_when_empty_for_rfc3339() {
        let wrapper = Wrapper {
            range: RangeQuery {
                min: None,
                max: None,
            },
        };
        let json = serde_json::to_string(&wrapper).unwrap();
        assert_eq!(json, r#"{"range":{"min":null,"max":null}}"#);
    }

    #[test]
    fn should_deserialize_rangequery_when_empty_for_rfc3339() {
        let json = r#"{"range":{"min":null,"max":null}}"#;
        let wrapper: Wrapper = serde_json::from_str(json).unwrap();
        assert_eq!(
            wrapper.range,
            RangeQuery {
                min: None,
                max: None
            }
        );
    }

    // --- serde tests for Option<RangeQuery<OffsetDateTime>> ---

    #[test]
    fn should_serialize_option_rangequery_when_none_for_rfc3339_option() {
        let wrapper = WrapperOpt { range: None };
        let json = serde_json::to_string(&wrapper).unwrap();
        assert_eq!(json, r#"{}"#);
    }

    #[test]
    fn should_serialize_option_rangequery_when_some_for_rfc3339_option() {
        let wrapper = WrapperOpt {
            range: Some(RangeQuery {
                min: Some(datetime!(2022-01-01 00:00 UTC)),
                max: Some(datetime!(2022-06-30 12:00 UTC)),
            }),
        };
        let json = serde_json::to_string(&wrapper).unwrap();
        assert_eq!(
            json,
            r#"{"range":{"min":"2022-01-01T00:00:00Z","max":"2022-06-30T12:00:00Z"}}"#
        );
    }

    #[test]
    fn should_deserialize_option_rangequery_when_null_for_rfc3339_option() {
        let json = r#"{}"#;
        let wrapper: WrapperOpt = serde_json::from_str(json).unwrap();
        assert_eq!(wrapper.range, None);
    }

    #[test]
    fn should_deserialize_option_rangequery_when_some_for_rfc3339_option() {
        let json = r#"{"range":{"min":"2022-01-01T00:00:00Z","max":"2022-06-30T12:00:00Z"}}"#;
        let wrapper: WrapperOpt = serde_json::from_str(json).unwrap();
        assert_eq!(
            wrapper.range,
            Some(RangeQuery {
                min: Some(datetime!(2022-01-01 00:00 UTC)),
                max: Some(datetime!(2022-06-30 12:00 UTC))
            })
        );
    }
}
