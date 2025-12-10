#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub from: u64,
    pub size: u64,
}

impl Default for Page {
    fn default() -> Self {
        Self { from: 0, size: 21 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaginatedResult<T> {
    pub items: Vec<T>,
    pub page: Page,
    pub total: Option<u64>,
}

impl<T> PaginatedResult<T> {
    pub fn map_item<U, F>(self, f: F) -> PaginatedResult<U>
    where
        F: FnMut(T) -> U,
    {
        PaginatedResult {
            items: self.items.into_iter().map(f).collect(),
            page: self.page,
            total: self.total,
        }
    }
}

impl<T> Default for PaginatedResult<T> {
    fn default() -> Self {
        Self {
            items: vec![],
            page: Default::default(),
            total: None,
        }
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{
            error::ApiError,
            error_code::{BAD_PAGE_FROM_VALUE, BAD_PAGE_SIZE_VALUE},
        },
        pagination::page::{Page, PaginatedResult},
    };
    use aws_lambda_events::query_map::QueryMap;
    use serde::{Deserialize, Serialize};

    pub fn extract_page_query(headers: &QueryMap) -> Result<Option<Page>, ApiError> {
        let from = headers
            .first("from")
            .map(str::trim)
            .map(|from| from.parse::<u64>())
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(BAD_PAGE_FROM_VALUE, Box::new(err))
                    .with_query_field("from")
                    .with_detail(msg)
            })?;
        let size = headers
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

        if let Some(from) = from
            && let Some(size) = size
        {
            Ok(Some(Page { from, size }))
        } else {
            Ok(None)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct PaginatedData<T> {
        pub items: Vec<T>,
        pub from: u64,
        pub size: u64,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub total: Option<u64>,
    }

    impl<T> From<PaginatedResult<T>> for PaginatedData<T> {
        fn from(result: PaginatedResult<T>) -> Self {
            PaginatedData {
                items: result.items,
                from: result.page.from,
                size: result.page.size,
                total: result.total,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use rstest;

        use crate::api::error::{ApiErrorSource, ApiErrorSourceType};
        use crate::api::error_code::{BAD_PAGE_FROM_VALUE, BAD_PAGE_SIZE_VALUE};
        use crate::pagination::page::Page;
        use crate::pagination::page::api::extract_page_query;
        use aws_lambda_events::query_map::QueryMap;
        use std::collections::HashMap;

        #[trace]
        #[rstest::rstest]
        #[case(Some("0"), Some("10"), Some(Page { from: 0, size: 10 }))]
        #[case(Some("10"), Some("10"), Some(Page { from: 10, size: 10 }))]
        #[case(Some("42"), Some("69"), Some(Page { from: 42, size: 69 }))]
        #[case(Some("69"), Some("37"), Some(Page { from: 69, size: 37 }))]
        #[case(Some(" 69 "), Some(" 37 "), Some(Page { from: 69, size: 37 }))]
        #[case(Some(" 69"), Some(" 37"), Some(Page { from: 69, size: 37 }))]
        #[case(Some("1"), Some("1"), Some(Page { from: 1, size: 1 }))]
        #[case::enforce_max_size(Some("7"), Some("65535"), Some(Page { from: 7, size: 100 }))]
        #[case(None, Some("1"), None)]
        #[case(None, Some("42"), None)]
        #[case(Some("31"), None, None)]
        #[case(None, None, None)]
        fn should_extract_page(
            #[case] from_value: Option<&str>,
            #[case] size_value: Option<&str>,
            #[case] expected: Option<Page>,
        ) {
            let mut map = HashMap::new();
            if let Some(from_value) = from_value {
                map.insert("from".to_string(), from_value.to_string());
            }
            if let Some(size_value) = size_value {
                map.insert("size".to_string(), size_value.to_string());
            }
            let query = QueryMap::from(map);

            let actual = extract_page_query(&query).unwrap();

            assert_eq!(expected, actual);
        }

        #[trace]
        #[rstest::rstest]
        #[case("boop")]
        #[case("foo")]
        #[case("bar")]
        #[case("1x")]
        #[case("07g")]
        fn should_400_when_from_is_invalid_for_u64(#[case] value: &str) {
            let mut map = HashMap::new();
            map.insert("from".to_string(), value.to_string());
            let query = QueryMap::from(map);

            let actual = extract_page_query(&query).unwrap_err();

            assert_eq!(400, actual.status);
            assert_eq!(BAD_PAGE_FROM_VALUE, actual.error);
            assert_eq!(
                Some(ApiErrorSource {
                    field: "from",
                    source_type: ApiErrorSourceType::Query,
                }),
                actual.source
            )
        }

        #[trace]
        #[rstest::rstest]
        #[case("boop")]
        #[case("foo")]
        #[case("bar")]
        #[case("1x")]
        #[case("07g")]
        fn should_400_when_size_is_invalid(#[case] value: &str) {
            let mut map = HashMap::new();
            map.insert("size".to_string(), value.to_string());
            let query = QueryMap::from(map);

            let actual = extract_page_query(&query).unwrap_err();

            assert_eq!(400, actual.status);
            assert_eq!(BAD_PAGE_SIZE_VALUE, actual.error);
            assert_eq!(
                Some(ApiErrorSource {
                    field: "size",
                    source_type: ApiErrorSourceType::Query,
                }),
                actual.source
            )
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::pagination::page::{Page, PaginatedResult};
    use fake::{Dummy, Fake, Faker, Rng};

    impl<T: Dummy<Faker>> Dummy<Faker> for PaginatedResult<T> {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let items: Vec<T> = config.fake_with_rng(rng);
            let page = Page {
                from: config.fake_with_rng(rng),
                size: config.fake_with_rng(rng),
            };
            PaginatedResult {
                items,
                page,
                total: config.fake_with_rng(rng),
            }
        }
    }
}
