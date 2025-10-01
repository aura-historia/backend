use async_trait::async_trait;
use common::{
    pagination::page::{Page, PaginatedResult},
    sort::Sort,
};
use shop_core::shop::Shop;
use shop_core::sort_shop_field::SortShopField;
use shop_opensearch::repository::ShopOpenSearchRepository;
use shop_opensearch::shop_search::ShopSearch;
use tracing::{error, warn};

#[derive(thiserror::Error, Debug)]
pub enum SearchShopsError {
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),
}

#[cfg(feature = "api")]
pub mod api {
    use crate::query_service::SearchShopsError;
    use common::api::error::ApiError;
    use common::api::error_code::INTERNAL_SERVER_ERROR;
    use tracing::error;

    impl From<SearchShopsError> for ApiError {
        fn from(err: SearchShopsError) -> Self {
            match err {
                SearchShopsError::OpenSearchError(err) => {
                    error!(error = ?err, "Encountered OpenSearchError while searching shops.");
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR)
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
        page: &Option<Page>,
    ) -> Result<PaginatedResult<Shop>, SearchShopsError>;
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
        page: &Option<Page>,
    ) -> Result<PaginatedResult<Shop>, SearchShopsError> {
        let search_response = self
            .repository
            .search_shop_documents(search, sort, page)
            .await?;

        if search_response.timed_out {
            warn!(
                searchFilter = ?search,
                sort = ?sort,
                page = ?page,
                took = search_response.took,
                shardStats = ?search_response.shards,
                "Search-Request to OpenSearch timed out when querying shops."
            );
        }

        let shops = search_response
            .hits
            .hits
            .into_iter()
            .map(|hit| hit.source)
            .map(Shop::from)
            .collect::<Vec<_>>();

        let from = page.map(|page| page.from).unwrap_or(0);
        let size = shops.len() as u64;
        Ok(PaginatedResult {
            items: shops,
            page: Page { from, size },
            total: Some(search_response.hits.total.value),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::query_service::{QueryShopService, QueryShopServiceImpl};
    use common::pagination::page::Page;
    use common::query::range_query::RangeQuery;
    use common::{
        opensearch::search_response::{
            HitsMetadata, SearchHit, SearchResponse, ShardStats, TotalHits,
        },
        sort::{Sort, SortOrder},
    };
    use serde::ser::Error;
    use shop_core::sort_shop_field::SortShopField;
    use shop_opensearch::repository::MockShopOpenSearchRepository;
    use shop_opensearch::shop_document::ShopDocument;
    use shop_opensearch::shop_search::ShopSearch;
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
        Some(Page { from: 0, size: 20 }),
        100
    )]
    #[case(
        ShopSearch {
            shop_name_query: Some("Woaaaah Co. Ltd.".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: None
        },
        Some(Sort { sort: SortShopField::Name, order: SortOrder::Desc }),
        Some(Page { from: 10, size: 30 }),
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
        Some(Page { from: 3, size: 5 }),
        222
    )]
    async fn should_search_shops(
        #[case] search: ShopSearch,
        #[case] sort: Option<Sort<SortShopField>>,
        #[case] page: Option<Page>,
        #[case] count: usize,
    ) {
        let mut repository = MockShopOpenSearchRepository::default();
        repository
            .expect_search_shop_documents()
            .return_once(move |_, _, _| {
                Box::pin(async move { Ok(mk_search_response(fake::vec![ShopDocument; count])) })
            });
        let service = QueryShopServiceImpl::new(&repository);

        let actual = service.search_shops(&search, &sort, &page).await.unwrap();

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
}
