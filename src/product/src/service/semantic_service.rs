use crate::dynamodb::repository::ProductDynamoDbRepository;
use crate::opensearch::repository::ProductOpenSearchRepository;
use crate::{core::item::LocalizedItemView, service::query_service::localize_item_document};
use async_trait::async_trait;
use aws_sdk_dynamodb::error::SdkError;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use time::OffsetDateTime;
use tracing::{error, warn};

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum SemanticSearchItemsError {
    #[error("Product with ShopId '{0}' and ShopsProductId '{1}' not found.")]
    ItemNotFound(ShopId, ShopsProductId),

    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(#[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::semantic_service::SemanticSearchItemsError;
    use common::api::error::ApiError;
    use common::api::error_code::{INTERNAL_SERVER_ERROR, ITEM_NOT_FOUND};

    impl From<SemanticSearchItemsError> for ApiError {
        fn from(err: SemanticSearchItemsError) -> Self {
            match err {
                SemanticSearchItemsError::ItemNotFound(_, _) => {
                    ApiError::not_found(ITEM_NOT_FOUND, Box::new(err))
                }
                SemanticSearchItemsError::OpenSearchError(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
                SemanticSearchItemsError::SdkGetItemError(sdk_error) => sdk_error.into(),
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait SemanticSearchService {
    async fn similar_items(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Option<Vec<LocalizedItemView>>, SemanticSearchItemsError>;
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
    async fn similar_items(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Option<Vec<LocalizedItemView>>, SemanticSearchItemsError> {
        let product_id = self
            .dynamodb_repository
            .get_item_id(shop_id, shops_product_id)
            .await?
            .ok_or(SemanticSearchItemsError::ItemNotFound(
                *shop_id,
                shops_product_id.clone(),
            ))?;
        let document = self.opensearch_repository.get_by_id(&product_id).await?;
        match document.text_embedding {
            None => {
                if OffsetDateTime::now_utc().date() > document.created.date() {
                    warn!(
                        shopId = %shop_id,
                        shopItemId = %shops_product_id,
                        itemId = %product_id,
                        "When trying to find similar items for given ProductId,
                         ProductDocument for ProductId did not have a textEmbedding
                         although it was created at least one day prior -
                         hence why the nightly item-enrichment SHOULD have run and embedded the text."
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
                    .filter(|hit| hit.source.product_id != document.product_id)
                    .map(|hit| hit.source)
                    .map(|doc| localize_item_document(doc, languages, currency))
                    .collect();
                Ok(Some(localized_documents))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        dynamodb::repository::MockItemDynamoDbRepository,
        opensearch::{item_document::ProductDocument, repository::MockItemOpenSearchRepository},
        service::semantic_service::{
            SemanticSearchItemsError, SemanticSearchService, SemanticSearchServiceImpl,
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
    use serde::ser::Error;
    use std::panic;

    #[tokio::test]
    async fn should_return_similar_items_when_text_embedding_exists() {
        let mut dynamodb_repository = MockItemDynamoDbRepository::default();
        let mut opensearch_repository = MockItemOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_item_id()
            .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
        opensearch_repository
            .expect_get_by_id()
            .return_once(|product_id| {
                let product_id = *product_id;
                Box::pin(async move {
                    let mut doc = Faker.fake::<ProductDocument>();
                    doc.product_id = product_id;
                    doc.text_embedding = Some(Faker.fake());
                    Ok(doc)
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
                                    index: "items".to_string(),
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
            .similar_items(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await
            .unwrap();
        assert_eq!(42, actual.unwrap().len());
    }

    #[tokio::test]
    async fn should_return_no_items_when_text_embedding_not_exists() {
        let mut dynamodb_repository = MockItemDynamoDbRepository::default();
        let mut opensearch_repository = MockItemOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_item_id()
            .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
        opensearch_repository
            .expect_get_by_id()
            .return_once(|product_id| {
                let product_id = *product_id;
                Box::pin(async move {
                    let mut doc = Faker.fake::<ProductDocument>();
                    doc.product_id = product_id;
                    Ok(doc)
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
                                    index: "items".to_string(),
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
            .similar_items(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await
            .unwrap();
        assert!(actual.is_none());
    }

    #[tokio::test]
    async fn should_err_item_not_found_when_item_not_exists() {
        let mut dynamodb_repository = MockItemDynamoDbRepository::default();
        let opensearch_repository = MockItemOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_item_id()
            .return_once(|_, _| Box::pin(async { Ok(None) }));

        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);
        let actual = service
            .similar_items(&shop_id, &shops_product_id, &[Faker.fake()], &Faker.fake())
            .await;
        match actual.unwrap_err() {
            SemanticSearchItemsError::ItemNotFound(err_shop_id, err_shops_item_id) => {
                assert_eq!(shop_id, err_shop_id);
                assert_eq!(shops_product_id, err_shops_item_id);
            }
            other => panic!("Expected 'SemanticSearchItemsError::ItemNotFound' but got '{other}'"),
        }
    }

    #[tokio::test]
    async fn should_filter_out_self() {
        let mut dynamodb_repository = MockItemDynamoDbRepository::default();
        let mut opensearch_repository = MockItemOpenSearchRepository::default();

        let product_id = ProductId::new();
        let mut root = Faker.fake::<ProductDocument>();
        root.product_id = product_id;
        root.text_embedding = Some(Faker.fake());
        let root_clone1 = root.clone();
        let root_clone2 = root.clone();

        dynamodb_repository
            .expect_get_item_id()
            .return_once(move |_, _| Box::pin(async move { Ok(Some(product_id)) }));
        opensearch_repository
            .expect_get_by_id()
            .return_once(|_| Box::pin(async move { Ok(root_clone1) }));
        opensearch_repository
            .expect_k_nn_text()
            .return_once(|_, _| {
                let mut documents = fake::vec![ProductDocument; 42];
                documents.push(root_clone2);
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
                                    index: "items".to_string(),
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
            .similar_items(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(42, actual.len());
        assert!(!actual.iter().any(|item| item.product_id == root.product_id));
    }

    #[tokio::test]
    #[rstest::rstest]
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
    async fn should_err_when_sdk_get_item_error(
        #[case] expected: SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>,
    ) {
        let mut dynamodb_repository = MockItemDynamoDbRepository::default();
        let opensearch_repository = MockItemOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_item_id()
            .return_once(|_, _| Box::pin(async { Err(expected) }));

        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);
        let actual = service
            .similar_items(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await;
        match actual.unwrap_err() {
            SemanticSearchItemsError::SdkGetItemError(_) => {}
            other => {
                panic!("Expected 'SemanticSearchItemsError::SdkGetItemError' but got '{other}'")
            }
        }
    }

    #[tokio::test]
    async fn should_err_when_item_document_not_exists() {
        let mut dynamodb_repository = MockItemDynamoDbRepository::default();
        let mut opensearch_repository = MockItemOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_item_id()
            .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
        opensearch_repository
            .expect_get_by_id()
            .return_once(|_| Box::pin(async { Err(serde_json::Error::custom("foo").into()) }));

        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);
        let actual = service
            .similar_items(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await;
        match actual.unwrap_err() {
            SemanticSearchItemsError::OpenSearchError(_) => {}
            other => {
                panic!("Expected 'SemanticSearchItemsError::OpenSearchError' but got '{other}'")
            }
        }
    }

    #[tokio::test]
    async fn should_err_when_knn_fails() {
        let mut dynamodb_repository = MockItemDynamoDbRepository::default();
        let mut opensearch_repository = MockItemOpenSearchRepository::default();

        dynamodb_repository
            .expect_get_item_id()
            .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
        opensearch_repository
            .expect_get_by_id()
            .return_once(|product_id| {
                let product_id = *product_id;
                Box::pin(async move {
                    let mut doc = Faker.fake::<ProductDocument>();
                    doc.product_id = product_id;
                    doc.text_embedding = Some(Faker.fake());
                    Ok(doc)
                })
            });
        opensearch_repository
            .expect_k_nn_text()
            .return_once(|_, _| Box::pin(async { Err(serde_json::Error::custom("foo").into()) }));

        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);
        let actual = service
            .similar_items(&Faker.fake(), &Faker.fake(), &[Faker.fake()], &Faker.fake())
            .await;
        match actual.unwrap_err() {
            SemanticSearchItemsError::OpenSearchError(_) => {}
            other => {
                panic!("Expected 'SemanticSearchItemsError::OpenSearchError' but got '{other}'")
            }
        }
    }
}
