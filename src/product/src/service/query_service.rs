use crate::core::product::LocalizedProductView;
use crate::core::product::Product;
use crate::core::product_search::ProductSearch;
use crate::core::sort_product_field::SortProductField;
use crate::opensearch::repository::ProductOpenSearchRepository;
use crate::service::hybrid_search::HybridSearchError;
use async_trait::async_trait;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::sort::{Sort, SortOrder};
use tracing::warn;

#[derive(thiserror::Error, Debug)]
pub enum SearchProductsError {
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),
    #[error("HybridSearchError: {0}")]
    HybridSearchError(#[from] HybridSearchError),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::hybrid_search::HybridSearchError;
    use crate::service::query_service::SearchProductsError;
    use common::api::error::ApiError;
    impl From<SearchProductsError> for ApiError {
        fn from(err: SearchProductsError) -> Self {
            match err {
                SearchProductsError::OpenSearchError(opensearch_err) => opensearch_err.into(),
                SearchProductsError::HybridSearchError(err) => match err {
                    HybridSearchError::OpenSearchError(opensearch_err) => opensearch_err.into(),
                },
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

    /// Adaptive hybrid retrieval: BM25 + kNN run server-side, ranked via Reciprocal Rank
    /// Fusion (RRF). The kNN side uses the supplied query `embedding` (computed by the caller
    /// via the existing [`product_pipeline_embed_text::service::MultimodalEmbeddingService`]).
    /// `vector_weight` and `candidate_k` are derived dynamically from soft intent signals
    /// over the textual query.
    ///
    /// `search.product_query` MUST be set; this method always uses `_score desc` ordering
    /// and is therefore unsuitable for searches with explicit non-score sort.
    /// Pagination is via `search_after = [fused_score]`; ties on score are non-deterministic.
    async fn search_products_with_dynamic_semantics(
        &self,
        search: &ProductSearch,
        embedding: &[f32],
        page: &Option<Cursor<serde_json::Value>>,
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
            .await?;
        let cursor = Cursor {
            size: search_response.hits.hits.len() as u64,
            search_after: search_response
                .hits
                .hits
                .last()
                .and_then(|last| last.sort.clone()),
        };
        if search_response.timed_out {
            warn!(
                searchFilter = ?search,
                sort = ?sort,
                page = ?page,
                took = search_response.took,
                shardStats = ?search_response.shards,
                "Search-Request to OpenSearch timed out when querying products."
            );
        }

        let product_views = search_response
            .hits
            .hits
            .into_iter()
            .map(|hit| hit.source)
            .map(Product::from)
            .map(|product| product.localized(&search.currency, &[search.language]))
            .collect::<Vec<_>>();

        Ok(CursoredResult {
            items: product_views,
            cursor,
            total: Some(search_response.hits.total.value),
        })
    }

    async fn search_products_with_dynamic_semantics(
        &self,
        search: &ProductSearch,
        embedding: &[f32],
        page: &Option<Cursor<serde_json::Value>>,
    ) -> Result<CursoredResult<LocalizedProductView, serde_json::Value>, SearchProductsError> {
        let outcome = crate::service::hybrid_search::hybrid_search(
            self.repository,
            search,
            embedding,
            page,
            &[search.language],
        )
        .await?;
        Ok(outcome.items)
    }
}

#[cfg(test)]
mod tests {
    use crate::core::product_search::ProductSearch;
    use crate::core::sort_product_field::SortProductField;
    use crate::opensearch::{
        product_document::ProductDocument, repository::MockProductOpenSearchRepository,
    };
    use crate::service::query_service::{QueryProductService, QueryProductServiceImpl};
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
            product_query: Some("Hallo Welt".try_into().unwrap()),
            category_id: Default::default(),
            period_id: Default::default(),
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
            origin_year_query: Default::default(),
            authenticity_query: Default::default(),
            condition_query: Default::default(),
            provenance_query: Default::default(),
            restoration_query: Default::default(),
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
            product_query: Some("Hallo Welt".try_into().unwrap()),
            category_id: Default::default(),
            period_id: Default::default(),
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
            origin_year_query: Default::default(),
            authenticity_query: Default::default(),
            condition_query: Default::default(),
            provenance_query: Default::default(),
            restoration_query: Default::default(),
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
            product_query: Some("Hallo Welten!".try_into().unwrap()),
            category_id: Default::default(),
            period_id: Default::default(),
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
            origin_year_query: Some(RangeQuery { min: None, max: Some(451.into()) }),
            authenticity_query: Default::default(),
            condition_query: Default::default(),
            provenance_query: Default::default(),
            restoration_query: Default::default(),
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
            product_query: Some("Hallo Welten!".try_into().unwrap()),
            category_id: Default::default(),
            period_id: Default::default(),
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
            origin_year_query: Some(RangeQuery { min: Some(152.into()), max: Some(1818.into()) }),
            authenticity_query: Default::default(),
            condition_query: Default::default(),
            provenance_query: Default::default(),
            restoration_query: Default::default(),
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
            product_query: Some("Hallo Welten!".try_into().unwrap()),
            category_id: Default::default(),
            period_id: Default::default(),
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
            origin_year_query: Some(RangeQuery { min: Some((-152).into()), max: None }),
            authenticity_query: Default::default(),
            condition_query: Default::default(),
            provenance_query: Default::default(),
            restoration_query: Default::default(),
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
            product_query: None,
            category_id: Default::default(),
            period_id: Default::default(),
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
            origin_year_query: Some(RangeQuery { min: Some((-152).into()), max: None }),
            authenticity_query: Default::default(),
            condition_query: Default::default(),
            provenance_query: Default::default(),
            restoration_query: Default::default(),
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
                    product_query: Some("Hallo Welten!".try_into().unwrap()),
                    category_id: Default::default(),
                    period_id: Default::default(),
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
                    origin_year_query: None,
                    authenticity_query: Default::default(),
                    condition_query: Default::default(),
                    provenance_query: Default::default(),
                    restoration_query: Default::default(),
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
                    product_query: Some("Hallo Welten!".try_into().unwrap()),
                    category_id: Default::default(),
                    period_id: Default::default(),
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
                    origin_year_query: None,
                    authenticity_query: Default::default(),
                    condition_query: Default::default(),
                    provenance_query: Default::default(),
                    restoration_query: Default::default(),
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
                    product_query: Some("Hallo Welten!".try_into().unwrap()),
                    category_id: Default::default(),
                    period_id: Default::default(),
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
                    origin_year_query: None,
                    authenticity_query: Default::default(),
                    condition_query: Default::default(),
                    provenance_query: Default::default(),
                    restoration_query: Default::default(),
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
        assert!(
            actual
                .items
                .iter()
                .all(|item| item.description.clone().unwrap().localization == language)
        );
        assert!(
            actual
                .items
                .iter()
                .all(|item| item.description.clone().unwrap().payload.as_ref() == expected)
        );
    }
}
