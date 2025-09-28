use crate::paginated_result::PaginatedResult;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffsetLimitPaginatedData<T> {
    pub items: Vec<T>,
    pub pagination: PaginationData<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeySetTimePaginatedData<T> {
    pub items: Vec<T>,

    #[serde(with = "crate::api::collection::pagination_rfc3339")]
    pub pagination: PaginationData<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaginationData<Key> {
    pub from: Key,
    pub size: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Key>,
}

/// A serde adapter for `PaginationData<OffsetDateTime>` using RFC3339.
pub mod pagination_rfc3339 {
    use super::PaginationData;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use time::OffsetDateTime;

    pub fn serialize<S>(
        data: &PaginationData<OffsetDateTime>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Helper {
            #[serde(with = "time::serde::rfc3339")]
            from: OffsetDateTime,
            size: u64,
            total: Option<u64>,
            #[serde(with = "time::serde::rfc3339::option")]
            next: Option<OffsetDateTime>,
        }

        let helper = Helper {
            from: data.from,
            size: data.size,
            total: data.total,
            next: data.next,
        };
        helper.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PaginationData<OffsetDateTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            #[serde(with = "time::serde::rfc3339")]
            from: OffsetDateTime,
            size: u64,
            total: Option<u64>,
            #[serde(with = "time::serde::rfc3339::option")]
            next: Option<OffsetDateTime>,
        }

        let helper = Helper::deserialize(deserializer)?;
        Ok(PaginationData {
            from: helper.from,
            size: helper.size,
            total: helper.total,
            next: helper.next,
        })
    }

    /// Support for `Option<PaginationData<OffsetDateTime>>`
    pub mod option {
        use super::*;
        use serde::{Deserializer, Serializer};

        pub fn serialize<S>(
            value: &Option<PaginationData<OffsetDateTime>>,
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
        ) -> Result<Option<PaginationData<OffsetDateTime>>, D::Error>
        where
            D: Deserializer<'de>,
        {
            Ok(Some(super::deserialize(deserializer)?))
        }
    }
}

impl<T> From<PaginatedResult<T, u64>> for OffsetLimitPaginatedData<T> {
    fn from(paginated: PaginatedResult<T, u64>) -> Self {
        OffsetLimitPaginatedData {
            items: paginated.items,
            pagination: PaginationData {
                from: paginated.page.from,
                size: paginated.page.size,
                total: paginated.total,
                next: paginated.next_after,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutCollectionData<T> {
    pub items: Vec<T>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{self, json};
    use time::macros::datetime;

    #[test]
    fn should_serialize_deserialize_keyset_time_pagination_data() {
        let data = KeySetTimePaginatedData {
            items: vec!["a".to_string(), "b".to_string()],
            pagination: PaginationData {
                from: datetime!(2023-05-01 12:00:00 UTC),
                size: 10,
                total: Some(25),
                next: Some(datetime!(2023-05-02 12:00:00 UTC)),
            },
        };
        let expected = json!({
            "items": ["a","b"],
            "pagination": {
                "from": "2023-05-01T12:00:00Z",
                "size": 10,
                "total": 25,
                "next": "2023-05-02T12:00:00Z"
            }
        });

        let actual = serde_json::to_value(&data).unwrap();
        assert_eq!(expected, actual);

        let deserialized: KeySetTimePaginatedData<String> = serde_json::from_value(actual).unwrap();
        assert_eq!(data, deserialized);
    }
}
