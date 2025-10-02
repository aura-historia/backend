#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor<C> {
    pub size: u64,
    pub search_after: Option<C>,
}

impl<C> Default for Cursor<C> {
    fn default() -> Self {
        Self {
            size: 21,
            search_after: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursoredResult<T, C> {
    pub items: Vec<T>,
    pub cursor: Cursor<C>,
    pub total: Option<u64>,
}

impl<T, C> CursoredResult<T, C> {
    pub fn map_item<U, F>(self, f: F) -> CursoredResult<U, C>
    where
        F: FnMut(T) -> U,
    {
        CursoredResult {
            items: self.items.into_iter().map(f).collect(),
            cursor: self.cursor,
            total: self.total,
        }
    }
}

impl<T, C> Default for CursoredResult<T, C> {
    fn default() -> Self {
        Self {
            items: vec![],
            cursor: Default::default(),
            total: None,
        }
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{
            error::ApiError,
            error_code::{BAD_PAGE_SIZE_VALUE, INVALID_JSON, INVALID_RFC3339_TIMESTAMP},
        },
        pagination::cursor::{Cursor, CursoredResult},
    };
    use aws_lambda_events::query_map::QueryMap;
    use serde::{Deserialize, Serialize};
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    pub fn extract_time_cursor_query(
        headers: &QueryMap,
    ) -> Result<Option<Cursor<OffsetDateTime>>, ApiError> {
        let search_after = headers
            .first("searchAfter")
            .map(str::trim)
            .map(|val| OffsetDateTime::parse(val, &Rfc3339))
            .transpose()
            .map_err(|err| {
                ApiError::bad_request(INVALID_RFC3339_TIMESTAMP)
                    .with_query_field("searchAfter")
                    .with_message(err.to_string())
            })?;
        let size = headers
            .first("size")
            .map(str::trim)
            .map(|size| size.parse::<u64>())
            .transpose()
            .map_err(|err| {
                ApiError::bad_request(BAD_PAGE_SIZE_VALUE)
                    .with_query_field("size")
                    .with_message(err.to_string())
            })?
            .map(|size| size.min(100));

        if let Some(size) = size {
            Ok(Some(Cursor { search_after, size }))
        } else {
            Ok(None)
        }
    }

    pub fn extract_json_cursor_query(
        headers: &QueryMap,
    ) -> Result<Option<Cursor<serde_json::Value>>, ApiError> {
        let search_after = headers
            .first("searchAfter")
            .map(str::trim)
            .map(serde_json::from_str)
            .transpose()
            .map_err(|err| {
                ApiError::bad_request(INVALID_JSON)
                    .with_query_field("searchAfter")
                    .with_message(err.to_string())
            })?;
        let size = headers
            .first("size")
            .map(str::trim)
            .map(|size| size.parse::<u64>())
            .transpose()
            .map_err(|err| {
                ApiError::bad_request(BAD_PAGE_SIZE_VALUE)
                    .with_query_field("size")
                    .with_message(err.to_string())
            })?
            .map(|size| size.min(100));

        if let Some(size) = size {
            Ok(Some(Cursor { search_after, size }))
        } else {
            Ok(None)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct TimeCursoredData<T> {
        pub items: Vec<T>,
        pub size: u64,

        #[serde(
            with = "time::serde::rfc3339::option",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        pub search_after: Option<OffsetDateTime>,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub total: Option<u64>,
    }

    impl<T> From<CursoredResult<T, OffsetDateTime>> for TimeCursoredData<T> {
        fn from(result: CursoredResult<T, OffsetDateTime>) -> Self {
            TimeCursoredData {
                items: result.items,
                size: result.cursor.size,
                search_after: result.cursor.search_after,
                total: result.total,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct JsonCursoredData<T> {
        pub items: Vec<T>,
        pub size: u64,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub search_after: Option<serde_json::Value>,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub total: Option<u64>,
    }

    impl<T> From<CursoredResult<T, serde_json::Value>> for JsonCursoredData<T> {
        fn from(result: CursoredResult<T, serde_json::Value>) -> Self {
            JsonCursoredData {
                items: result.items,
                size: result.cursor.size,
                search_after: result.cursor.search_after,
                total: result.total,
            }
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::pagination::cursor::{Cursor, CursoredResult};
    use fake::{Dummy, Fake, Faker, Rng};
    use time::OffsetDateTime;

    impl<T: Dummy<Faker>> Dummy<Faker> for CursoredResult<T, OffsetDateTime> {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let items: Vec<T> = config.fake_with_rng(rng);
            let cursor = Cursor {
                search_after: if config.fake_with_rng(rng) {
                    Some(OffsetDateTime::now_utc())
                } else {
                    None
                },
                size: config.fake_with_rng(rng),
            };
            CursoredResult {
                items,
                cursor,
                total: config.fake_with_rng(rng),
            }
        }
    }
}
