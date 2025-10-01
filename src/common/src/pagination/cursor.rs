#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor<C> {
    pub size: u64,
    pub search_after: Option<C>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursoredResult<T, C> {
    pub items: Vec<T>,
    pub cursor: Cursor<C>,
    pub total: Option<u64>,
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{
            error::ApiError,
            error_code::{BAD_PAGE_SIZE_VALUE, INVALID_RFC3339_TIMESTAMP},
        },
        pagination::cursor::Cursor,
    };
    use aws_lambda_events::query_map::QueryMap;
    use serde::{Deserialize, Serialize};
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    pub fn extract_cursor_query(
        headers: &QueryMap,
    ) -> Result<Option<Cursor<OffsetDateTime>>, ApiError> {
        let search_after = headers
            .first("from")
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
}
