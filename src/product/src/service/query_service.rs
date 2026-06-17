use crate::core::product::LocalizedProductView;
use crate::core::product::Product;
use crate::core::product_search::ProductSearch;
use crate::core::sort_product_field::SortProductField;
use crate::opensearch::product_document::ProductDocument;
use crate::opensearch::repository::ProductOpenSearchRepository;
use async_trait::async_trait;
use common::opensearch::search_response::{OpenSearchTimedOutError, SearchResponse};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::sort::{Sort, SortOrder};

#[derive(thiserror::Error, Debug)]
pub enum SearchProductsError {
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),
    #[error("OpenSearchTimedOut: {0}")]
    OpenSearchTimedOut(#[from] OpenSearchTimedOutError),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::query_service::SearchProductsError;
    use common::api::error::ApiError;
    impl From<SearchProductsError> for ApiError {
        fn from(err: SearchProductsError) -> Self {
            match err {
                SearchProductsError::OpenSearchError(opensearch_err) => opensearch_err.into(),
                SearchProductsError::OpenSearchTimedOut(timeout_err) => timeout_err.into(),
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait QueryProductService {
    async fn search_products(
        &self,
        search: &ProductSearch,
        sort: &Option<Sort<SortProductField>>,
        page: &Option<Cursor<serde_json::Value>>,
    ) -> Result<CursoredResult<LocalizedProductView, serde_json::Value>, SearchProductsError>;

    /// Run OpenSearch-native hybrid search (BM25 + kNN) combined by the configured RRF
    /// search pipeline.
    ///
    /// `search.product_query` MUST be set; this method always uses relevance ordering and is
    /// therefore unsuitable for searches with explicit non-score sort. Pagination uses the
    /// raw OpenSearch `search_after` cursor from the hybrid response.
    async fn search_products_hybrid(
        &self,
        search: &ProductSearch,
        embedding: &[f32],
        page: &Option<Cursor<serde_json::Value>>,
    ) -> Result<CursoredResult<LocalizedProductView, serde_json::Value>, SearchProductsError>;

    async fn search_products_with_percolator_query(
        &self,
        search: &ProductSearch,
        size: u64,
    ) -> Result<CursoredResult<LocalizedProductView, serde_json::Value>, SearchProductsError>;
}

pub struct QueryProductServiceImpl<'a> {
    repository: &'a (dyn ProductOpenSearchRepository + Sync),
}

impl<'a> QueryProductServiceImpl<'a> {
    pub fn new(repository: &'a (dyn ProductOpenSearchRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl<'a> QueryProductService for QueryProductServiceImpl<'a> {
    async fn search_products(
        &self,
        search: &ProductSearch,
        sort: &Option<Sort<SortProductField>>,
        page: &Option<Cursor<serde_json::Value>>,
    ) -> Result<CursoredResult<LocalizedProductView, serde_json::Value>, SearchProductsError> {
        let search_response = self
            .repository
            .search_product_documents(
                search,
                &sort.unwrap_or(Sort {
                    sort: SortProductField::Score,
                    order: SortOrder::Desc,
                }),
                page,
            )
            .await?
            .into_non_timed_out("product search")?;

        Ok(map_search_response(search, search_response))
    }

    async fn search_products_hybrid(
        &self,
        search: &ProductSearch,
        embedding: &[f32],
        page: &Option<Cursor<serde_json::Value>>,
    ) -> Result<CursoredResult<LocalizedProductView, serde_json::Value>, SearchProductsError> {
        let search_response = self
            .repository
            .hybrid_search_product_documents(search, embedding, page)
            .await?
            .into_non_timed_out("product hybrid search")?;

        Ok(map_hybrid_search_response(search, search_response, page))
    }

    async fn search_products_with_percolator_query(
        &self,
        search: &ProductSearch,
        size: u64,
    ) -> Result<CursoredResult<LocalizedProductView, serde_json::Value>, SearchProductsError> {
        let search_response = self
            .repository
            .search_product_documents_with_percolator_query(search, size)
            .await?
            .into_non_timed_out("product percolator-preview search")?;

        let mut result = map_search_response(search, search_response);
        result.cursor.search_after = None;
        Ok(result)
    }
}

fn map_search_response(
    search: &ProductSearch,
    search_response: SearchResponse<ProductDocument>,
) -> CursoredResult<LocalizedProductView, serde_json::Value> {
    let cursor = Cursor {
        size: search_response.hits.hits.len() as u64,
        search_after: search_response
            .hits
            .hits
            .last()
            .and_then(|last| last.sort.clone()),
    };

    let product_views = map_product_views(search, search_response.hits.hits);

    CursoredResult {
        items: product_views,
        cursor,
        total: Some(search_response.hits.total.value),
    }
}

fn map_hybrid_search_response(
    search: &ProductSearch,
    search_response: SearchResponse<ProductDocument>,
    page: &Option<Cursor<serde_json::Value>>,
) -> CursoredResult<LocalizedProductView, serde_json::Value> {
    let requested_size = page.as_ref().map(|cursor| cursor.size).unwrap_or(20).max(1);
    let page_size = search_response.hits.hits.len() as u64;
    let search_after = if page_size >= requested_size {
        search_response
            .hits
            .hits
            .last()
            .and_then(hybrid_search_after)
    } else {
        None
    };

    CursoredResult {
        items: map_product_views(search, search_response.hits.hits),
        cursor: Cursor {
            size: page_size,
            search_after,
        },
        // Native hybrid `hits.total` is bounded by the fused candidate pool (for example
        // by kNN `k`), so exposing it as a product result count is misleading.
        total: None,
    }
}

fn hybrid_search_after(
    hit: &common::opensearch::search_response::SearchHit<ProductDocument>,
) -> Option<serde_json::Value> {
    hit.sort.clone().or_else(|| {
        hit.score
            .filter(|score| score.is_finite())
            .map(|score| serde_json::json!([score]))
    })
}

fn map_product_views(
    search: &ProductSearch,
    hits: Vec<common::opensearch::search_response::SearchHit<ProductDocument>>,
) -> Vec<LocalizedProductView> {
    hits.into_iter()
        .map(|hit| hit.source)
        .map(Product::from)
        .map(|product| product.localized(&search.currency, &[search.language]))
        .collect::<Vec<_>>()
}

#[cfg(test)]
mod tests {
    use crate::core::product_search::ProductSearch;
    use crate::core::sort_product_field::SortProductField;
    use crate::opensearch::{
        product_document::ProductDocument, repository::MockProductOpenSearchRepository,
    };
    use crate::service::query_service::{
        QueryProductService, QueryProductServiceImpl, SearchProductsError,
    };
    use common::language::document::{LanguageDocument, TextDocument};
    use common::pagination::cursor::Cursor;
    use common::query::any_of_query::AnyOfQuery;
    use common::query::range_query::RangeQuery;
    use common::{
        currency::domain::Currency,
        language::domain::Language,
        opensearch::search_response::{
            HitsMetadata, SearchHit, SearchResponse, ShardStats, TotalHits,
        },
        product_state::domain::ProductState,
        sort::{Sort, SortOrder},
    };
    use fake::{Fake, Faker};
    use rstest;
    use serde::ser::Error;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    fn mk_search_response(
        product_documents: Vec<ProductDocument>,
    ) -> SearchResponse<ProductDocument> {
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
                    value: product_documents.len() as u64,
                    relation: "eq".to_string(),
                },
                max_score: None,
                hits: product_documents
                    .into_iter()
                    .map(|product_document| SearchHit {
                        index: "products".to_string(),
                        id: product_document.product_id.to_string(),
                        score: None,
                        source: product_document,
                        sort: None,
                        matched_queries: vec![],
                    })
                    .collect(),
            },
        }
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(
        ProductSearch {
            language: Language::De,
            currency: Currency::Eur,
            product_query: vec!["Hallo Welt".try_into().unwrap()],
            enhanced_search_description: None,
            shop_name_query: HashSet::from_iter(["Hallo Shop".into()]).into(),
            exclude_shop_name_query: Default::default(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: Default::default(),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: Some(RangeQuery { min: Some(100u64.into()), max: Some(999999u64.into()) }),
            state_query: AnyOfQuery::from(HashSet::from_iter([ProductState::Available, ProductState::Listed])),
            created_query: Some(RangeQuery { min: Some(datetime!(1000-01-01 0:00 UTC)), max: Some(datetime!(3000-01-01 0:00 UTC)) }),
            updated_query: Some(RangeQuery { min: Some(datetime!(1000-01-01 0:00 UTC)), max: Some(datetime!(3000-01-01 0:00 UTC)) }),
            auction_start_query: None,
            auction_end_query: None,
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
        },
        Some(Sort { sort: SortProductField::Price, order: SortOrder::Asc }),
        Some(Cursor { search_after: None, size: 20 }),
        100
    )]
    #[case(
        ProductSearch {
            language: Language::En,
            currency: Currency::Usd,
            product_query: vec!["Hallo Welt".try_into().unwrap()],
            enhanced_search_description: None,
            shop_name_query: HashSet::from_iter(["Hallo Shop".into()]).into(),
            exclude_shop_name_query: HashSet::from_iter(["Hallo Welt".into()]).into(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: Default::default(),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: Some(RangeQuery { min: Some(100u64.into()), max: Some(999999u64.into()) }),
            state_query: AnyOfQuery::from(HashSet::from_iter([ProductState::Available, ProductState::Listed])),
            created_query: Some(RangeQuery { min: Some(datetime!(1000-01-01 0:00 UTC)), max: Some(datetime!(3000-01-01 0:00 UTC)) }),
            updated_query: Some(RangeQuery { min: Some(datetime!(1000-01-01 0:00 UTC)), max: Some(datetime!(3000-01-01 0:00 UTC)) }),
            auction_start_query: None,
            auction_end_query: None,
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
        },
        Some(Sort { sort: SortProductField::Price, order: SortOrder::Desc }),
        Some(Cursor { search_after: Some(json!([12345, "foo"])), size: 20 }),
        500
    )]
    #[case(
        ProductSearch {
            language: Language::En,
            currency: Currency::Gbp,
            product_query: vec!["Hallo Welten!".try_into().unwrap()],
            enhanced_search_description: None,
            shop_name_query: Default::default(),
            exclude_shop_name_query: Default::default(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: Default::default(),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: Some(RangeQuery { min: Some(100000u64.into()), max: Some(999999004u64.into()) }),
            state_query: AnyOfQuery::from(HashSet::from_iter([ProductState::Available, ProductState::Listed])),
            created_query: Some(RangeQuery { min: None, max: Some(datetime!(3000-01-01 0:00 UTC)) }),
            updated_query: Some(RangeQuery { min: Some(datetime!(1000-01-01 0:00 UTC)), max: None }),
            auction_start_query: None,
            auction_end_query: None,
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
        },
        None,
        None,
        1111
    )]
    #[case(
        ProductSearch {
            language: Language::Fr,
            currency: Currency::Eur,
            product_query: vec!["Hallo Welten!".try_into().unwrap()],
            enhanced_search_description: None,
            shop_name_query: Default::default(),
            exclude_shop_name_query: Default::default(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: Default::default(),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: None,
            state_query: Default::default(),
            created_query: None,
            updated_query: None,
            auction_start_query: None,
            auction_end_query: None,
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
        },
        None,
        None,
        123
    )]
    #[case(
        ProductSearch {
            language: Language::Es,
            currency: Currency::Eur,
            product_query: vec!["Hallo Welten!".try_into().unwrap()],
            enhanced_search_description: None,
            shop_name_query: Default::default(),
            exclude_shop_name_query: Default::default(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: Default::default(),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: None,
            state_query: Default::default(),
            created_query: None,
            updated_query: None,
            auction_start_query: None,
            auction_end_query: None,
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
        },
        None,
        None,
        1234
    )]
    #[case(
        ProductSearch {
            language: Language::Es,
            currency: Currency::Eur,
            product_query: Vec::new(),
            enhanced_search_description: None,
            shop_name_query: Default::default(),
            exclude_shop_name_query: Default::default(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: Default::default(),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: None,
            state_query: Default::default(),
            created_query: None,
            updated_query: None,
            auction_start_query: None,
            auction_end_query: None,
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
        },
        None,
        None,
        1234
    )]
    #[trace]
    async fn should_search_items(
        #[case] search: ProductSearch,
        #[case] sort: Option<Sort<SortProductField>>,
        #[case] page: Option<Cursor<serde_json::Value>>,
        #[case] count: usize,
    ) {
        let mut repository = MockProductOpenSearchRepository::default();
        repository
            .expect_search_product_documents()
            .return_once(move |_, _, _| {
                Box::pin(async move { Ok(mk_search_response(fake::vec![ProductDocument; count])) })
            });
        let service = QueryProductServiceImpl::new(&repository);

        let actual = service
            .search_products(&search, &sort, &page)
            .await
            .unwrap();

        assert_eq!(count, actual.items.len());
        assert_eq!(count, actual.total.unwrap() as usize);
    }

    #[tokio::test]
    async fn should_propagate_opensearch_error() {
        let mut repository = MockProductOpenSearchRepository::default();
        repository
            .expect_search_product_documents()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Err(opensearch::Error::from(serde_json::Error::custom(
                        "Something went wrong.",
                    )))
                })
            });
        let service = QueryProductServiceImpl::new(&repository);

        let actual = service
            .search_products(
                &ProductSearch {
                    language: Language::De,
                    currency: Currency::Cad,
                    product_query: vec!["Hallo Welten!".try_into().unwrap()],
                    enhanced_search_description: None,
                    shop_name_query: Default::default(),
                    exclude_shop_name_query: Default::default(),
                    seller_name_query: Default::default(),
                    exclude_seller_name_query: Default::default(),
                    shop_type_query: Default::default(),
                    country_query: Default::default(),
                    continent_query: Default::default(),
                    geo_address_distance_query: None,
                    price_query: None,
                    state_query: Default::default(),
                    created_query: None,
                    updated_query: None,
                    auction_start_query: None,
                    auction_end_query: None,
                    shop_slug_id_query: Default::default(),
                    exclude_shop_slug_id_query: Default::default(),
                    seller_slug_id_query: Default::default(),
                    exclude_seller_slug_id_query: Default::default(),
                },
                &None,
                &None,
            )
            .await;

        assert!(actual.is_err());
    }

    #[tokio::test]
    async fn should_err_when_product_search_times_out() {
        let mut repository = MockProductOpenSearchRepository::default();
        repository
            .expect_search_product_documents()
            .return_once(|_, _, _| {
                Box::pin(async move {
                    Ok(SearchResponse {
                        took: 250,
                        timed_out: true,
                        shards: ShardStats {
                            total: 4,
                            successful: 3,
                            skipped: 0,
                            failed: 1,
                        },
                        hits: HitsMetadata {
                            total: TotalHits {
                                value: 0,
                                relation: "eq".to_string(),
                            },
                            max_score: None,
                            hits: vec![],
                        },
                    })
                })
            });
        let service = QueryProductServiceImpl::new(&repository);

        let actual = service
            .search_products(&Faker.fake(), &None, &None)
            .await
            .unwrap_err();

        assert!(matches!(actual, SearchProductsError::OpenSearchTimedOut(_)));
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case::eur(Currency::Eur, 2)]
    #[case::gbp(Currency::Gbp, 4)]
    #[case::usd(Currency::Usd, 10)]
    #[case::aud(Currency::Aud, 1000)]
    #[case::cad(Currency::Cad, 4000)]
    #[case::nzd(Currency::Nzd, 42)]
    #[trace]
    async fn should_respect_currency(#[case] currency: Currency, #[case] expected_amount: u64) {
        let mut repository = MockProductOpenSearchRepository::default();
        repository
            .expect_search_product_documents()
            .return_once(move |_, _, _| {
                let items = fake::vec![ProductDocument; 369]
                    .into_iter()
                    .map(|mut product| {
                        product.price_eur = Some(2);
                        product.price_gbp = Some(4);
                        product.price_usd = Some(10);
                        product.price_aud = Some(1000);
                        product.price_cad = Some(4000);
                        product.price_nzd = Some(42);
                        product
                    })
                    .collect();
                Box::pin(async move { Ok(mk_search_response(items)) })
            });
        let service = QueryProductServiceImpl::new(&repository);

        let actual = service
            .search_products(
                &ProductSearch {
                    language: Language::De,
                    currency,
                    product_query: vec!["Hallo Welten!".try_into().unwrap()],
                    enhanced_search_description: None,
                    shop_name_query: Default::default(),
                    exclude_shop_name_query: Default::default(),
                    seller_name_query: Default::default(),
                    exclude_seller_name_query: Default::default(),
                    shop_type_query: Default::default(),
                    country_query: Default::default(),
                    continent_query: Default::default(),
                    geo_address_distance_query: None,
                    price_query: None,
                    state_query: Default::default(),
                    created_query: None,
                    updated_query: None,
                    auction_start_query: None,
                    auction_end_query: None,
                    shop_slug_id_query: Default::default(),
                    exclude_shop_slug_id_query: Default::default(),
                    seller_slug_id_query: Default::default(),
                    exclude_seller_slug_id_query: Default::default(),
                },
                &None,
                &None,
            )
            .await
            .unwrap();

        assert!(
            actual
                .items
                .iter()
                .map(|item| item.price.unwrap())
                .all(|price| price.currency == currency
                    && price.monetary_amount == expected_amount.into())
        );
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(Language::De, "German")]
    #[case(Language::En, "English")]
    #[case(Language::Fr, "French")]
    #[case(Language::Es, "Spanish")]
    #[trace]
    async fn should_respect_language(#[case] language: Language, #[case] expected: &str) {
        let mut repository = MockProductOpenSearchRepository::default();
        repository
            .expect_search_product_documents()
            .return_once(move |_, _, _| {
                let items = fake::vec![ProductDocument; 369]
                    .into_iter()
                    .map(|mut product| {
                        product.title_native = TextDocument {
                            text: "German".to_string(),
                            language: LanguageDocument::De,
                        };
                        product.title_de = Some("German".to_string());
                        product.title_en = Some("English".to_string());
                        product.title_fr = Some("French".to_string());
                        product.title_es = Some("Spanish".to_string());
                        product
                    })
                    .collect();
                Box::pin(async move { Ok(mk_search_response(items)) })
            });
        let service = QueryProductServiceImpl::new(&repository);

        let actual = service
            .search_products(
                &ProductSearch {
                    language,
                    currency: Currency::Aud,
                    product_query: vec!["Hallo Welten!".try_into().unwrap()],
                    enhanced_search_description: None,
                    shop_name_query: Default::default(),
                    exclude_shop_name_query: Default::default(),
                    seller_name_query: Default::default(),
                    exclude_seller_name_query: Default::default(),
                    shop_type_query: Default::default(),
                    country_query: Default::default(),
                    continent_query: Default::default(),
                    geo_address_distance_query: None,
                    price_query: None,
                    state_query: Default::default(),
                    created_query: None,
                    updated_query: None,
                    auction_start_query: None,
                    auction_end_query: None,
                    shop_slug_id_query: Default::default(),
                    exclude_shop_slug_id_query: Default::default(),
                    seller_slug_id_query: Default::default(),
                    exclude_seller_slug_id_query: Default::default(),
                },
                &None,
                &None,
            )
            .await
            .unwrap();

        assert!(
            actual
                .items
                .iter()
                .all(|item| item.title.localization == language)
        );
        assert!(
            actual
                .items
                .iter()
                .all(|item| { item.title.payload.as_ref() == expected })
        );
    }

    #[tokio::test]
    async fn should_search_items_with_hybrid_search() {
        let mut repository = MockProductOpenSearchRepository::default();
        repository
            .expect_hybrid_search_product_documents()
            .return_once(|_, _, _| {
                Box::pin(async move { Ok(mk_search_response(fake::vec![ProductDocument; 7])) })
            });
        let service = QueryProductServiceImpl::new(&repository);

        let actual = service
            .search_products_hybrid(&Faker.fake(), &[0.1_f32; 3], &None)
            .await
            .unwrap();

        assert_eq!(7, actual.items.len());
        assert!(actual.total.is_none());
        assert!(actual.cursor.search_after.is_none());
    }

    #[tokio::test]
    async fn should_return_search_after_from_score_when_hybrid_hit_sort_is_missing() {
        let mut repository = MockProductOpenSearchRepository::default();
        repository
            .expect_hybrid_search_product_documents()
            .return_once(|_, _, _| {
                let mut response = mk_search_response(fake::vec![ProductDocument; 2]);
                response.hits.hits[0].score = Some(0.42);
                response.hits.hits[1].score = Some(0.21);
                Box::pin(async move { Ok(response) })
            });
        let service = QueryProductServiceImpl::new(&repository);

        let actual = service
            .search_products_hybrid(
                &Faker.fake(),
                &[0.1_f32; 3],
                &Some(Cursor {
                    size: 2,
                    search_after: None,
                }),
            )
            .await
            .unwrap();

        assert_eq!(Some(json!([0.21])), actual.cursor.search_after);
        assert!(actual.total.is_none());
    }

    #[tokio::test]
    async fn should_err_when_product_hybrid_search_times_out() {
        let mut repository = MockProductOpenSearchRepository::default();
        repository
            .expect_hybrid_search_product_documents()
            .return_once(|_, _, _| {
                Box::pin(async move {
                    Ok(SearchResponse {
                        took: 250,
                        timed_out: true,
                        shards: ShardStats {
                            total: 4,
                            successful: 3,
                            skipped: 0,
                            failed: 1,
                        },
                        hits: HitsMetadata {
                            total: TotalHits {
                                value: 0,
                                relation: "eq".to_string(),
                            },
                            max_score: None,
                            hits: vec![],
                        },
                    })
                })
            });
        let service = QueryProductServiceImpl::new(&repository);

        let actual = service
            .search_products_hybrid(&Faker.fake(), &[0.1_f32; 3], &None)
            .await
            .unwrap_err();

        assert!(matches!(actual, SearchProductsError::OpenSearchTimedOut(_)));
    }
}
