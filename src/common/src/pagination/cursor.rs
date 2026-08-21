// Legacy shim. Owner: application. Remove after legacy common consumers migrate.
pub use application::pagination::{Cursor, CursoredResult};

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
    use serde_json::Value;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    pub fn extract_time_cursor_query(
        query: &QueryMap,
    ) -> Result<Option<Cursor<OffsetDateTime>>, ApiError> {
        let search_after = query
            .first("searchAfter")
            .map(str::trim)
            .map(|val| OffsetDateTime::parse(val, &Rfc3339))
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(INVALID_RFC3339_TIMESTAMP, Box::new(err))
                    .with_query_field("searchAfter")
                    .with_detail(msg)
            })?;
        let size = query
            .first("size")
            .map(str::trim)
            .map(|size| size.parse::<u64>())
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(BAD_PAGE_SIZE_VALUE, Box::new(err))
                    .with_query_field("size")
                    .with_detail(msg)
            })?
            .map(|size| size.min(100));

        if let Some(size) = size {
            Ok(Some(Cursor { search_after, size }))
        } else {
            Ok(None)
        }
    }

    pub fn extract_json_cursor_query(
        query: &QueryMap,
    ) -> Result<Option<Cursor<serde_json::Value>>, ApiError> {
        let mut search_after_vals = query
            .all("searchAfter")
            .unwrap_or_default()
            .into_iter()
            .map(str::trim)
            .try_fold(
                Vec::new(),
                |mut acc: Vec<Value>, el_str| -> Result<Vec<Value>, ApiError> {
                    let el_str = match el_str {
                        "null" => el_str,
                        "true" => el_str,
                        "false" => el_str,
                        s if s.starts_with("[") || s.starts_with("{") => el_str,
                        s if s.parse::<u64>().is_ok() || s.parse::<f64>().is_ok() => el_str,
                        string => &format!("\"{string}\""),
                    };
                    let el = serde_json::from_str(el_str).map_err(|err| {
                        let msg = format!("Failed parsing '{el_str}' as JSON-Value: {err}",);
                        ApiError::bad_request(INVALID_JSON, Box::new(err))
                            .with_query_field("searchAfter")
                            .with_detail(msg)
                    })?;
                    acc.push(el);
                    Ok(acc)
                },
            )?;
        let search_after = match search_after_vals.len() {
            0 => None,
            1 => Some(search_after_vals.remove(0)),
            _ => {
                let search_after = serde_json::to_value(search_after_vals).map_err(|err| {
                    let msg = format!("Failed parsing 'searchAfter' as JSON-Array: {err}",);
                    ApiError::bad_request(INVALID_JSON, Box::new(err))
                        .with_query_field("searchAfter")
                        .with_detail(msg)
                })?;
                Some(search_after)
            }
        };
        let size = query
            .first("size")
            .map(str::trim)
            .map(|size| size.parse::<u64>())
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(BAD_PAGE_SIZE_VALUE, Box::new(err))
                    .with_query_field("size")
                    .with_detail(msg)
            })?
            .map(|size| size.min(100));

        if size.is_some() || search_after.is_some() {
            Ok(Some(Cursor {
                search_after,
                size: size.unwrap_or_else(|| Cursor::<Value>::default().size),
            }))
        } else {
            Ok(None)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
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
    #[serde(rename_all = "camelCase")]
    pub struct JsonCursoredData<T> {
        pub items: Vec<T>,
        pub size: u64,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub search_after: Option<serde_json::Value>,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub total: Option<u64>,
    }

    impl<T, TData> From<CursoredResult<T, serde_json::Value>> for JsonCursoredData<TData>
    where
        T: Into<TData>,
    {
        fn from(result: CursoredResult<T, serde_json::Value>) -> Self {
            JsonCursoredData {
                items: result.items.into_iter().map(Into::into).collect(),
                size: result.cursor.size,
                search_after: result.cursor.search_after,
                total: result.total,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use rstest;

        use crate::pagination::cursor::{Cursor, api::extract_json_cursor_query};
        use aws_lambda_events::query_map::QueryMap;
        use serde_json::{Value, json};
        use std::collections::HashMap;

        #[rstest::rstest]
        #[trace]
        #[case([].into(), None)]
        #[case([("searchAfter".to_owned(), vec!["5".to_owned()])].into(), Some(Cursor { size: 21, search_after: Some(json!(5)) }))]
        #[case([("size".to_owned(), vec!["10".to_owned()]), ("searchAfter".to_owned(), vec!["5".to_owned()])].into(), Some(Cursor { size: 10, search_after: Some(json!(5)) }))]
        #[case([("size".to_owned(), vec!["10".to_owned()]), ("searchAfter".to_owned(), vec!["6ba7b810-9dad-11d1-80b4-00c04fd430c8".to_owned()])].into(), Some(Cursor { size: 10, search_after: Some(json!("6ba7b810-9dad-11d1-80b4-00c04fd430c8")) }))]
        #[case([("size".to_owned(), vec!["10".to_owned()]), ("searchAfter".to_owned(), vec!["5".to_owned(), "42". to_owned()])].into(), Some(Cursor { size: 10, search_after: Some(json!([5, 42])) }))]
        #[case([("size".to_owned(), vec!["10".to_owned()]), ("searchAfter".to_owned(), vec!["5".to_owned(), "6ba7b810-9dad-11d1-80b4-00c04fd430c8". to_owned()])].into(), Some(Cursor { size: 10, search_after: Some(json!([5, "6ba7b810-9dad-11d1-80b4-00c04fd430c8"])) }))]
        fn should_extract_json_cursor_from_query(
            #[case] query_map: HashMap<String, Vec<String>>,
            #[case] expected: Option<Cursor<Value>>,
        ) {
            let query = QueryMap::from(query_map);

            let actual = extract_json_cursor_query(&query).unwrap();
            assert_eq!(expected, actual);
        }
    }
}
