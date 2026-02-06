use crate::core::product::{LocalizedProductView, Product};
use crate::dynamodb::repository::ProductDynamoDbRepository;
use crate::opensearch::repository::ProductOpenSearchRepository;
use async_trait::async_trait;
use aws_sdk_dynamodb::error::SdkError;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use time::OffsetDateTime;
use tracing::warn;

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum SemanticSearchProductsError {
    #[error("Product with ShopId '{0}' and ShopsProductId '{1}' not found.")]
    ProductNotFound(ShopId, ShopsProductId),

    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(#[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::semantic_service::SemanticSearchProductsError;
    use common::api::error::ApiError;
    use common::api::error_code::PRODUCT_NOT_FOUND;

    impl From<SemanticSearchProductsError> for ApiError {
        fn from(err: SemanticSearchProductsError) -> Self {
            match err {
                SemanticSearchProductsError::ProductNotFound(_, _) => {
                    ApiError::not_found(PRODUCT_NOT_FOUND, Box::new(err))
                }
                SemanticSearchProductsError::OpenSearchError(opensearch_err) => {
                    opensearch_err.into()
                }
                SemanticSearchProductsError::SdkGetItemError(sdk_error) => sdk_error.into(),
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait SemanticSearchService {
    async fn similar_products(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Option<Vec<LocalizedProductView>>, SemanticSearchProductsError>;
}

pub struct SemanticSearchServiceImpl<'a> {
    dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
    opensearch_repository: &'a (dyn ProductOpenSearchRepository + Sync),
}

impl<'a> SemanticSearchServiceImpl<'a> {
    pub fn new(
        dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
        opensearch_repository: &'a (dyn ProductOpenSearchRepository + Sync),
    ) -> Self {
        Self {
            dynamodb_repository,
            opensearch_repository,
        }
    }
}

#[async_trait]
impl<'a> SemanticSearchService for SemanticSearchServiceImpl<'a> {
    async fn similar_products(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Option<Vec<LocalizedProductView>>, SemanticSearchProductsError> {
        let record = self
            .dynamodb_repository
            .get_product_record(shop_id, shops_product_id)
            .await?
            .ok_or(SemanticSearchProductsError::ProductNotFound(
                *shop_id,
                shops_product_id.clone(),
            ))?;
        match record.text_embedding {
            None => {
                if OffsetDateTime::now_utc().date() > record.created.date() {
                    warn!(
                        shopId = %shop_id,
                        shopProductId = %shops_product_id,
                        productId = %record.product_id,
                        "When trying to find similar products for given ProductKey,
                         ProductRecord for ProductKey did not have a textEmbedding
                         although it was created at least one day prior -
                         hence why the nightly product-enrichment SHOULD have run and embedded the text."
                    );
                }
                Ok(None)
            }
            Some(text_embedding) => {
                let localized_documents = self
                    .opensearch_repository
                    .k_nn_text(&text_embedding, 20)
                    .await?
                    .hits
                    .hits
                    .into_iter()
                    .filter(|hit| hit.source.product_id != record.product_id)
                    .map(|hit| hit.source)
                    .map(Product::from)
                    .map(|product| product.localized(currency, languages))
                    .collect();
                Ok(Some(localized_documents))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        dynamodb::{product_record::ProductRecord, repository::MockProductDynamoDbRepository},
        opensearch::{
            product_document::ProductDocument, repository::MockProductOpenSearchRepository,
        },
        service::semantic_service::{
            SemanticSearchProductsError, SemanticSearchService, SemanticSearchServiceImpl,
        },
    };
    use aws_sdk_dynamodb::config::http::HttpResponse;
    use aws_sdk_dynamodb::error::ConnectorError;
    use aws_sdk_dynamodb::error::SdkError;
    use common::{
        opensearch::search_response::{
            HitsMetadata, SearchHit, SearchResponse, ShardStats, TotalHits,
        },
        product_id::ProductId,
        shop_id::ShopId,
        shops_product_id::ShopsProductId,
    };
    use fake::{Fake, Faker};
    use rstest;
    use serde::ser::Error;
    use std::panic;

    #[tokio::test]
    async fn should_return_similar_products_when_text_embedding_exists() {
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_product_record()
            .return_once(|_, _| {
                Box::pin(async move {
                    let mut record = Faker.fake::<ProductRecord>();
                    record.text_embedding = Some(Faker.fake());
                    Ok(Some(record))
                })
            });
        opensearch_repository
            .expect_k_nn_text()
            .return_once(|_, _| {
                Box::pin(async {
                    Ok(SearchResponse {
                        took: 187,
                        timed_out: false,
                        shards: ShardStats {
                            total: 1,
                            successful: 1,
                            skipped: 0,
                            failed: 0,
                        },
                        hits: HitsMetadata {
                            total: TotalHits {
                                value: 42,
                                relation: "eq".to_string(),
                            },
                            max_score: None,
                            hits: fake::vec![ProductDocument; 42]
                                .into_iter()
                                .map(|doc| SearchHit {
                                    index: "products".to_string(),
                                    id: doc.product_id.to_string(),
                                    score: None,
                                    sort: None,
                                    source: doc,
                                })
                                .collect(),
                        },
                    })
                })
            });

        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);
        let actual = service
            .similar_products(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await
            .unwrap();
        assert_eq!(42, actual.unwrap().len());
    }

    #[tokio::test]
    async fn should_return_no_products_when_text_embedding_not_exists() {
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_product_record()
            .return_once(|_, _| {
                Box::pin(async move {
                    let mut record = Faker.fake::<ProductRecord>();
                    record.text_embedding = None;
                    Ok(Some(record))
                })
            });
        opensearch_repository
            .expect_k_nn_text()
            .return_once(|_, _| {
                Box::pin(async {
                    Ok(SearchResponse {
                        took: 187,
                        timed_out: false,
                        shards: ShardStats {
                            total: 1,
                            successful: 1,
                            skipped: 0,
                            failed: 0,
                        },
                        hits: HitsMetadata {
                            total: TotalHits {
                                value: 42,
                                relation: "eq".to_string(),
                            },
                            max_score: None,
                            hits: fake::vec![ProductDocument; 42]
                                .into_iter()
                                .map(|doc| SearchHit {
                                    index: "products".to_string(),
                                    id: doc.product_id.to_string(),
                                    score: None,
                                    sort: None,
                                    source: doc,
                                })
                                .collect(),
                        },
                    })
                })
            });

        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);
        let actual = service
            .similar_products(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await
            .unwrap();
        assert!(actual.is_none());
    }

    #[tokio::test]
    async fn should_err_product_not_found_when_product_not_exists() {
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        let opensearch_repository = MockProductOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_product_record()
            .return_once(|_, _| Box::pin(async { Ok(None) }));

        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);
        let actual = service
            .similar_products(&shop_id, &shops_product_id, &[Faker.fake()], &Faker.fake())
            .await;
        match actual.unwrap_err() {
            SemanticSearchProductsError::ProductNotFound(err_shop_id, err_shops_product_id) => {
                assert_eq!(shop_id, err_shop_id);
                assert_eq!(shops_product_id, err_shops_product_id);
            }
            other => {
                panic!("Expected 'SemanticSearchProductsError::ProductNotFound' but got '{other}'")
            }
        }
    }

    #[tokio::test]
    async fn should_filter_out_self() {
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();

        let product_id = ProductId::new();
        let mut root = Faker.fake::<ProductRecord>();
        root.product_id = product_id;
        root.text_embedding = Some(Faker.fake());
        let root_clone1 = root.clone();
        let root_clone2 = root.clone();

        dynamodb_repository
            .expect_get_product_record()
            .return_once(|_, _| Box::pin(async move { Ok(Some(root_clone1)) }));
        opensearch_repository
            .expect_k_nn_text()
            .return_once(|_, _| {
                let mut documents = fake::vec![ProductDocument; 42];
                documents.push(root_clone2.into());
                Box::pin(async move {
                    Ok(SearchResponse {
                        took: 187,
                        timed_out: false,
                        shards: ShardStats {
                            total: 1,
                            successful: 1,
                            skipped: 0,
                            failed: 0,
                        },
                        hits: HitsMetadata {
                            total: TotalHits {
                                value: 42,
                                relation: "eq".to_string(),
                            },
                            max_score: None,
                            hits: documents
                                .into_iter()
                                .map(|doc| SearchHit {
                                    index: "products".to_string(),
                                    id: doc.product_id.to_string(),
                                    score: None,
                                    sort: None,
                                    source: doc,
                                })
                                .collect(),
                        },
                    })
                })
            });

        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);
        let actual = service
            .similar_products(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(42, actual.len());
        assert!(!actual.iter().any(|item| item.product_id == root.product_id));
    }

    #[tokio::test]
    #[rstest::rstest]
    #[trace]
    #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
    #[case::timeout(SdkError::timeout_error("Something went wrong"))]
    #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
    #[case::response_error(SdkError::response_error(
        "Something went wrong",
        HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
    ))]
    #[case::service_error(SdkError::service_error(
        aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
        HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
    ))]
    async fn should_err_when_sdk_get_product_error(
        #[case] expected: SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>,
    ) {
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        let opensearch_repository = MockProductOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_product_record()
            .return_once(|_, _| Box::pin(async { Err(expected) }));

        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);
        let actual = service
            .similar_products(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await;
        match actual.unwrap_err() {
            SemanticSearchProductsError::SdkGetItemError(_) => {}
            other => {
                panic!("Expected 'SemanticSearchProductsError::SdkGetItemError' but got '{other}'")
            }
        }
    }

    #[tokio::test]
    async fn should_err_when_product_record_not_exists() {
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        let opensearch_repository = MockProductOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_product_record()
            .return_once(|_, _| Box::pin(async { Ok(None) }));

        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);
        let actual = service
            .similar_products(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await;
        match actual.unwrap_err() {
            SemanticSearchProductsError::ProductNotFound(_, _) => {}
            other => {
                panic!("Expected 'SemanticSearchProductsError::ProductNotFound' but got '{other}'")
            }
        }
    }

    #[tokio::test]
    async fn should_err_when_knn_fails() {
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_product_record()
            .return_once(|_, _| {
                Box::pin(async move {
                    let mut record = Faker.fake::<ProductRecord>();
                    record.text_embedding = Some(Faker.fake());
                    Ok(Some(record))
                })
            });
        opensearch_repository
            .expect_k_nn_text()
            .return_once(|_, _| Box::pin(async { Err(serde_json::Error::custom("foo").into()) }));

        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);
        let actual = service
            .similar_products(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await;
        match actual.unwrap_err() {
            SemanticSearchProductsError::OpenSearchError(_) => {}
            other => {
                panic!("Expected 'SemanticSearchProductsError::OpenSearchError' but got '{other}'")
            }
        }
    }
}
