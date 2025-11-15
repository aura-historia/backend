use crate::core::shop::Shop;
use crate::core::sort_shop_field::SortShopField;
use crate::opensearch::repository::ShopOpenSearchRepository;
use crate::opensearch::shop_search::ShopSearch;
use async_trait::async_trait;
use common::{
    pagination::cursor::{Cursor, CursoredResult},
    sort::{Sort, SortOrder},
};
use tracing::{error, warn};

#[derive(thiserror::Error, Debug)]
pub enum SearchShopsError {
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::query_service::SearchShopsError;
    use common::api::error::ApiError;
    use common::api::error_code::INTERNAL_SERVER_ERROR;

    impl From<SearchShopsError> for ApiError {
        fn from(err: SearchShopsError) -> Self {
            match err {
                SearchShopsError::OpenSearchError(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait QueryShopService {
    async fn search_shops(
        &self,
        search: &ShopSearch,
        sort: &Option<Sort<SortShopField>>,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<CursoredResult<Shop, serde_json::Value>, SearchShopsError>;
}

pub struct QueryShopServiceImpl<'a> {
    repository: &'a (dyn ShopOpenSearchRepository + Sync),
}

impl<'a> QueryShopServiceImpl<'a> {
    pub fn new(repository: &'a (dyn ShopOpenSearchRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl<'a> QueryShopService for QueryShopServiceImpl<'a> {
    async fn search_shops(
        &self,
        search: &ShopSearch,
        sort: &Option<Sort<SortShopField>>,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<CursoredResult<Shop, serde_json::Value>, SearchShopsError> {
        let sort = (*sort).unwrap_or(Sort {
            sort: SortShopField::Score,
            order: SortOrder::Desc,
        });
        let sort = if search.shop_name_query.is_none() && matches!(sort.sort, SortShopField::Score)
        {
            Sort {
                sort: SortShopField::Name,
                order: SortOrder::Asc,
            }
        } else {
            sort
        };

        let search_response = self
            .repository
            .search_shop_documents(search, &sort, cursor)
            .await?;
        if search_response.timed_out {
            warn!(
                searchFilter = ?search,
                sort = ?sort,
                cursor = ?cursor,
                took = search_response.took,
                shardStats = ?search_response.shards,
                "Search-Request to OpenSearch timed out when querying shops."
            );
        }

        let cursor = Cursor {
            size: search_response.hits.hits.len() as u64,
            search_after: search_response
                .hits
                .hits
                .last()
                .and_then(|last| last.sort.clone()),
        };

        let shops = search_response
            .hits
            .hits
            .into_iter()
            .map(|hit| hit.source)
            .map(Shop::from)
            .collect::<Vec<_>>();

        Ok(CursoredResult {
            items: shops,
            cursor,
            total: Some(search_response.hits.total.value),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::core::sort_shop_field::SortShopField;
    use crate::opensearch::repository::MockShopOpenSearchRepository;
    use crate::opensearch::shop_document::ShopDocument;
    use crate::opensearch::shop_search::ShopSearch;
    use crate::service::query_service::{QueryShopService, QueryShopServiceImpl};
    use common::pagination::cursor::Cursor;
    use common::query::range_query::RangeQuery;
    use common::shop_id::ShopId;
    use common::{
        opensearch::search_response::{
            HitsMetadata, SearchHit, SearchResponse, ShardStats, TotalHits,
        },
        sort::{Sort, SortOrder},
    };
    use serde::ser::Error;
    use serde_json::json;
    use time::macros::datetime;

    fn mk_search_response(shop_documents: Vec<ShopDocument>) -> SearchResponse<ShopDocument> {
        SearchResponse {
            took: 42,
            timed_out: false,
            shards: ShardStats {
                total: 5,
                successful: 4,
                skipped: 1,
                failed: 0,
            },
            hits: HitsMetadata {
                total: TotalHits {
                    value: shop_documents.len() as u64,
                    relation: "eq".to_string(),
                },
                max_score: None,
                hits: shop_documents
                    .into_iter()
                    .map(|shop_document| SearchHit {
                        index: "shops".to_string(),
                        id: shop_document.shop_id.to_string(),
                        score: None,
                        source: shop_document,
                        sort: None,
                    })
                    .collect(),
            },
        }
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(
        ShopSearch {
            shop_name_query: Some("Woaaaah Co. Ltd.".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) })
        },
        Some(Sort { sort: SortShopField::Created, order: SortOrder::Asc }),
        Some(Cursor { size: 20, search_after: Some(json!(["2021-01-01T00:00:00Z", ShopId::new()])) }),
        100
    )]
    #[case(
        ShopSearch {
            shop_name_query: Some("Woaaaah Co. Ltd.".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: None
        },
        Some(Sort { sort: SortShopField::Name, order: SortOrder::Desc }),
        Some(Cursor { size: 50, search_after: Some(json!(["Woaaaah Co. Ltd. and partners", ShopId::new()])) }),
        500
    )]
    #[case(
        ShopSearch {
            shop_name_query: Some("Woaaaah Co. Ltd.".try_into().unwrap()),
            created: None,
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) })
        },
        None,
        None,
        1111
    )]
    #[case(
        ShopSearch {
            shop_name_query: Some("Woaaaah Co. Ltd.".try_into().unwrap()),
            created: None,
            updated: None
        },
        None,
        None,
        123
    )]
    #[case(Default::default(), None, None, 222)]
    #[case(
        Default::default(),
        Some(Sort { sort: SortShopField::Updated, order: SortOrder::Desc }),
        Some(Cursor { size: 10, search_after: None }),
        222
    )]
    async fn should_search_shops(
        #[case] search: ShopSearch,
        #[case] sort: Option<Sort<SortShopField>>,
        #[case] cursor: Option<Cursor<serde_json::Value>>,
        #[case] count: usize,
    ) {
        let mut repository = MockShopOpenSearchRepository::default();
        repository
            .expect_search_shop_documents()
            .return_once(move |_, _, _| {
                Box::pin(async move { Ok(mk_search_response(fake::vec![ShopDocument; count])) })
            });
        let service = QueryShopServiceImpl::new(&repository);

        let actual = service.search_shops(&search, &sort, &cursor).await.unwrap();

        assert_eq!(count, actual.items.len());
        assert_eq!(count, actual.total.unwrap() as usize);
    }

    #[tokio::test]
    async fn should_propagate_opensearch_error() {
        let mut repository = MockShopOpenSearchRepository::default();
        repository
            .expect_search_shop_documents()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Err(opensearch::Error::from(serde_json::Error::custom(
                        "Something went wrong.",
                    )))
                })
            });
        let service = QueryShopServiceImpl::new(&repository);

        let actual = service
            .search_shops(
                &ShopSearch {
                    shop_name_query: Some("foobar".try_into().unwrap()),
                    created: None,
                    updated: None,
                },
                &None,
                &None,
            )
            .await;

        assert!(actual.is_err());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(
        ShopSearch {
            shop_name_query: None,
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: None
        },
        Some(Sort { sort: SortShopField::Score, order: SortOrder::Asc }),
    )]
    #[case(
        ShopSearch {
            shop_name_query: None,
            created: None,
            updated: None
        },
        Some(Sort { sort: SortShopField::Score, order: SortOrder::Desc }),
    )]
    #[case(
        ShopSearch {
            shop_name_query: None,
            created: None,
            updated: None
        },
        None,
    )]
    async fn should_default_sort_name_asc_when_empty_query_and_sort_score(
        #[case] search: ShopSearch,
        #[case] sort: Option<Sort<SortShopField>>,
    ) {
        let mut repository = MockShopOpenSearchRepository::default();
        repository
            .expect_search_shop_documents()
            .return_once(move |_, sort, _| {
                assert!(sort.sort == SortShopField::Name);
                assert!(sort.order == SortOrder::Asc);
                Box::pin(async move { Ok(mk_search_response(fake::vec![ShopDocument; 42])) })
            });
        let service = QueryShopServiceImpl::new(&repository);

        let _ = service.search_shops(&search, &sort, &None).await.unwrap();
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(
        ShopSearch {
            shop_name_query: Some("Woaaaah Co. Ltd.".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) })
        },
        Sort { sort: SortShopField::Created, order: SortOrder::Asc }
    )]
    #[case(
        ShopSearch {
            shop_name_query: Some("Woaaaah Co. Ltd.".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: None
        },
        Sort { sort: SortShopField::Name, order: SortOrder::Desc }
    )]
    async fn should_preserve_sort_when_empty_query_with_non_score(
        #[case] search: ShopSearch,
        #[case] in_sort: Sort<SortShopField>,
    ) {
        let mut repository = MockShopOpenSearchRepository::default();
        repository
            .expect_search_shop_documents()
            .return_once(move |_, arg_sort, _| {
                assert_eq!(in_sort, *arg_sort);
                Box::pin(async move { Ok(mk_search_response(fake::vec![ShopDocument; 42])) })
            });
        let service = QueryShopServiceImpl::new(&repository);

        let _ = service
            .search_shops(&search, &Some(in_sort), &None)
            .await
            .unwrap();
    }
}
