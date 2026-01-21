use crate::core::authenticity::Authenticity;
use crate::core::condition::Condition;
use crate::core::origin_year::OriginYear;
use crate::core::product_image::ProductImage;
use crate::core::product_search::ProductSearch;
use crate::core::provenance::Provenance;
use crate::core::restoration::Restoration;
use crate::core::sort_product_field::SortProductField;
use crate::core::{description::Description, product::LocalizedProductView, title::Title};
use crate::opensearch::product_document::ProductDocument;
use crate::opensearch::repository::ProductOpenSearchRepository;
use async_trait::async_trait;
use common::language::domain::Language;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::price::domain::Price;
use common::sort::{Sort, SortOrder};
use common::year::YearRange;
use common::{currency::domain::Currency, localized::Localized};
use std::collections::HashMap;
use strum::EnumCount;
use tracing::{error, warn};

#[derive(thiserror::Error, Debug)]
pub enum SearchProductsError {
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::query_service::SearchProductsError;
    use common::api::error::ApiError;
    impl From<SearchProductsError> for ApiError {
        fn from(err: SearchProductsError) -> Self {
            match err {
                SearchProductsError::OpenSearchError(opensearch_err) => opensearch_err.into(),
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
            .map(|product_document| {
                localize_product_document(product_document, &[search.language], &search.currency)
            })
            .collect::<Vec<_>>();

        Ok(CursoredResult {
            items: product_views,
            cursor,
            total: Some(search_response.hits.total.value),
        })
    }
}

pub fn localize_product_document(
    product_document: ProductDocument,
    languages: &[Language],
    currency: &Currency,
) -> LocalizedProductView {
    let mut available_titles: HashMap<Language, Title> = HashMap::with_capacity(Language::COUNT);
    available_titles.insert(
        product_document.title_native.language.into(),
        product_document.title_native.text.into(),
    );
    if let Some(title_de) = product_document.title_de {
        available_titles.insert(Language::De, title_de.into());
    }
    if let Some(title_en) = product_document.title_en {
        available_titles.insert(Language::En, title_en.into());
    }
    if let Some(title_fr) = product_document.title_fr {
        available_titles.insert(Language::Fr, title_fr.into());
    }
    if let Some(title_es) = product_document.title_es {
        available_titles.insert(Language::Es, title_es.into());
    }

    let mut available_descriptions: HashMap<Language, Description> =
        HashMap::with_capacity(Language::COUNT);
    if let Some(description_de) = product_document.description_de {
        available_descriptions.insert(Language::De, description_de.into());
    }
    if let Some(description_en) = product_document.description_en {
        available_descriptions.insert(Language::En, description_en.into());
    }
    if let Some(description_fr) = product_document.description_fr {
        available_descriptions.insert(Language::Fr, description_fr.into());
    }
    if let Some(description_es) = product_document.description_es {
        available_descriptions.insert(Language::Es, description_es.into());
    }

    let title = Language::resolve(languages, available_titles).unwrap_or_else(|| {
        error!(
            shopId = %product_document.shop_id,
            shopsProductId = %product_document.shops_product_id,
            "Failed resolving title. This SHOULD be impossible because the native title always exists."
        );
        Localized::new(Language::En, "Unknown title".into())
    });
    let description = Language::resolve(languages, available_descriptions);

    let price = match currency {
        Currency::Eur => product_document
            .price_eur
            .map(|amount| Price::new(amount.into(), Currency::Eur)),
        Currency::Gbp => product_document
            .price_gbp
            .map(|amount| Price::new(amount.into(), Currency::Gbp)),
        Currency::Usd => product_document
            .price_usd
            .map(|amount| Price::new(amount.into(), Currency::Usd)),
        Currency::Aud => product_document
            .price_aud
            .map(|amount| Price::new(amount.into(), Currency::Aud)),
        Currency::Cad => product_document
            .price_cad
            .map(|amount| Price::new(amount.into(), Currency::Cad)),
        Currency::Nzd => product_document
            .price_nzd
            .map(|amount| Price::new(amount.into(), Currency::Nzd)),
    };

    let price_estimate_min = match currency {
        Currency::Eur => product_document
            .price_estimate_min_eur
            .map(|amount| Price::new(amount.into(), Currency::Eur)),
        Currency::Gbp => product_document
            .price_estimate_min_gbp
            .map(|amount| Price::new(amount.into(), Currency::Gbp)),
        Currency::Usd => product_document
            .price_estimate_min_usd
            .map(|amount| Price::new(amount.into(), Currency::Usd)),
        Currency::Aud => product_document
            .price_estimate_min_aud
            .map(|amount| Price::new(amount.into(), Currency::Aud)),
        Currency::Cad => product_document
            .price_estimate_min_cad
            .map(|amount| Price::new(amount.into(), Currency::Cad)),
        Currency::Nzd => product_document
            .price_estimate_min_nzd
            .map(|amount| Price::new(amount.into(), Currency::Nzd)),
    };

    let price_estimate_max = match currency {
        Currency::Eur => product_document
            .price_estimate_max_eur
            .map(|amount| Price::new(amount.into(), Currency::Eur)),
        Currency::Gbp => product_document
            .price_estimate_max_gbp
            .map(|amount| Price::new(amount.into(), Currency::Gbp)),
        Currency::Usd => product_document
            .price_estimate_max_usd
            .map(|amount| Price::new(amount.into(), Currency::Usd)),
        Currency::Aud => product_document
            .price_estimate_max_aud
            .map(|amount| Price::new(amount.into(), Currency::Aud)),
        Currency::Cad => product_document
            .price_estimate_max_cad
            .map(|amount| Price::new(amount.into(), Currency::Cad)),
        Currency::Nzd => product_document
            .price_estimate_max_nzd
            .map(|amount| Price::new(amount.into(), Currency::Nzd)),
    };

    let state = product_document.state.into();

    LocalizedProductView {
        product_slug_id: product_document.product_slug_id,
        shop_slug_id: product_document.shop_slug_id,
        product_id: product_document.product_id,
        event_id: product_document.event_id,
        shop_id: product_document.shop_id,
        shops_product_id: product_document.shops_product_id,
        shop_name: product_document.shop_name.into(),
        shop_type: product_document.shop_type.into(),
        title,
        description,
        price,
        price_estimate_min,
        price_estimate_max,
        state,
        url: product_document.url,
        images: product_document
            .images
            .into_iter()
            .map(ProductImage::from)
            .collect(),
        origin_year: match (
            product_document.origin_year,
            product_document.origin_year_min,
            product_document.origin_year_max,
        ) {
            (None, None, None) => None,
            (Some(exact_year), _, _) => Some(OriginYear::ExactYear(exact_year)),
            (_, min, max) => Some(OriginYear::EstimatedRange(YearRange { min, max })),
        },
        authenticity: product_document.authenticity.map(Authenticity::from),
        condition: product_document.condition.map(Condition::from),
        provenance: product_document.provenance.map(Provenance::from),
        restoration: product_document.restoration.map(Restoration::from),
        auction_start: product_document.auction_start,
        auction_end: product_document.auction_end,
        created: product_document.created,
        updated: product_document.updated,
        history: None,
    }
}

#[cfg(test)]
mod tests {
    use rstest;

    use crate::core::product_search::ProductSearch;
    use crate::core::sort_product_field::SortProductField;
    use crate::opensearch::{
        product_document::ProductDocument, repository::MockProductOpenSearchRepository,
    };
    use crate::service::query_service::{QueryProductService, QueryProductServiceImpl};
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
            product_query: "Hallo Welt".try_into().unwrap(),
            shop_name_query: HashSet::from_iter(["Hallo Shop".into()]).into(),
            exclude_shop_name_query: Default::default(),
            shop_type_query: Default::default(),
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
        },
        Some(Sort { sort: SortProductField::Price, order: SortOrder::Asc }),
        Some(Cursor { search_after: None, size: 20 }),
        100
    )]
    #[case(
        ProductSearch {
            language: Language::En,
            currency: Currency::Usd,
            product_query: "Hallo Welt".try_into().unwrap(),
            shop_name_query: HashSet::from_iter(["Hallo Shop".into()]).into(),
            exclude_shop_name_query: HashSet::from_iter(["Hallo Welt".into()]).into(),
            shop_type_query: Default::default(),
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
        },
        Some(Sort { sort: SortProductField::Price, order: SortOrder::Desc }),
        Some(Cursor { search_after: Some(json!([12345, "foo"])), size: 20 }),
        500
    )]
    #[case(
        ProductSearch {
            language: Language::En,
            currency: Currency::Gbp,
            product_query: "Hallo Welten!".try_into().unwrap(),
            shop_name_query: Default::default(),
            exclude_shop_name_query: Default::default(),
            shop_type_query: Default::default(),
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
        },
        None,
        None,
        1111
    )]
    #[case(
        ProductSearch {
            language: Language::Fr,
            currency: Currency::Eur,
            product_query: "Hallo Welten!".try_into().unwrap(),
            shop_name_query: Default::default(),
            exclude_shop_name_query: Default::default(),
            shop_type_query: Default::default(),
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
        },
        None,
        None,
        123
    )]
    #[case(
        ProductSearch {
            language: Language::Es,
            currency: Currency::Eur,
            product_query: "Hallo Welten!".try_into().unwrap(),
            shop_name_query: Default::default(),
            exclude_shop_name_query: Default::default(),
            shop_type_query: Default::default(),
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
                    product_query: "Hallo Welten!".try_into().unwrap(),
                    shop_name_query: Default::default(),
                    exclude_shop_name_query: Default::default(),
                    shop_type_query: Default::default(),
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
                    product_query: "Hallo Welten!".try_into().unwrap(),
                    shop_name_query: Default::default(),
                    exclude_shop_name_query: Default::default(),
                    shop_type_query: Default::default(),
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
                        product.title_de = Some("German".to_string());
                        product.title_en = Some("English".to_string());
                        product.title_fr = Some("French".to_string());
                        product.title_es = Some("Spanish".to_string());
                        product.description_de = Some("German".to_string());
                        product.description_en = Some("English".to_string());
                        product.description_fr = Some("French".to_string());
                        product.description_es = Some("Spanish".to_string());
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
                    product_query: "Hallo Welten!".try_into().unwrap(),
                    shop_name_query: Default::default(),
                    exclude_shop_name_query: Default::default(),
                    shop_type_query: Default::default(),
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
                .all(|item| item.title.localization == language
                    && item.title.payload.as_ref() == expected
                    && item.description.clone().unwrap().localization == language
                    && item.description.clone().unwrap().payload.as_ref() == expected)
        );
    }
}
