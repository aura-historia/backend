use crate::category::{
    category_search::CategorySearch,
    core::{Category, LocalizedCategory},
    dynamodb_repository::CategoryDynamoDbRepository,
    opensearch_repository::CategoryOpenSearchRepository,
    sort_category_field::SortCategoryField,
};
use aws_sdk_dynamodb::{
    error::SdkError,
    operation::{get_item::GetItemError, put_item::PutItemError, query::QueryError},
};
use common::{
    category_key::CategoryId,
    error::missing_field::MissingRequiredField,
    language::domain::Language,
    sort::{Sort, SortOrder},
};

#[derive(Debug, thiserror::Error)]
pub enum CategoryServiceError {
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),

    #[error("Category '{0}' does not exist")]
    CategoryNotExists(CategoryId),

    #[error("DynamoDbSdkPutItemError: {0}")]
    DynamoDbSdkPutItemError(#[from] SdkError<PutItemError>),

    #[error("DynamoDbSdkGetItemError: {0}")]
    DynamoDbSdkGetItemError(#[from] SdkError<GetItemError>),

    #[error("DynamoDbSdkQueryError: {0}")]
    DynamoDbSdkQueryError(#[from] SdkError<QueryError>),

    #[error("MappingError: Missing required field '{0}'")]
    MappingError(#[from] MissingRequiredField),
}

#[cfg(feature = "data")]
pub mod api {
    use super::CategoryServiceError;
    use common::api::error::ApiError;
    use common::api::error_code::{INTERNAL_SERVER_ERROR, NOT_FOUND};

    impl From<CategoryServiceError> for ApiError {
        fn from(err: CategoryServiceError) -> Self {
            match err {
                CategoryServiceError::CategoryNotExists(_) => {
                    ApiError::not_found(NOT_FOUND, Box::new(err))
                }
                CategoryServiceError::OpenSearchError(e) => e.into(),
                CategoryServiceError::DynamoDbSdkPutItemError(e) => e.into(),
                CategoryServiceError::DynamoDbSdkGetItemError(e) => e.into(),
                CategoryServiceError::DynamoDbSdkQueryError(e) => e.into(),
                CategoryServiceError::MappingError(e) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(e))
                }
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait CategoryService {
    async fn upsert_category(&self, category: Category) -> Result<Category, CategoryServiceError>;

    async fn find_category(
        &self,
        category_id: &CategoryId,
    ) -> Result<Category, CategoryServiceError>;

    async fn view_category(
        &self,
        category_id: &CategoryId,
        languages: &[Language],
    ) -> Result<LocalizedCategory, CategoryServiceError>;

    async fn find_similar(
        &self,
        embedding: &[f32],
        k: u16,
    ) -> Result<Vec<(Category, f64)>, opensearch::Error>;

    async fn search_categories(
        &self,
        search: &CategorySearch,
        sort: &Option<Sort<SortCategoryField>>,
    ) -> Result<Vec<LocalizedCategory>, CategoryServiceError>;

    async fn find_categories(&self) -> Result<Vec<Category>, CategoryServiceError>;

    async fn view_categories(
        &self,
        languages: &[Language],
    ) -> Result<Vec<LocalizedCategory>, CategoryServiceError>;
}

pub struct CategoryServiceImpl<'a> {
    pub dynamodb_repository: &'a (dyn CategoryDynamoDbRepository + Send + Sync),
    pub opensearch_search: &'a (dyn CategoryOpenSearchRepository + Send + Sync),
}

impl<'a> CategoryServiceImpl<'a> {
    pub fn new(
        dynamodb_repository: &'a (dyn CategoryDynamoDbRepository + Send + Sync),
        opensearch_search: &'a (dyn CategoryOpenSearchRepository + Send + Sync),
    ) -> Self {
        Self {
            dynamodb_repository,
            opensearch_search,
        }
    }
}

#[async_trait::async_trait]
impl<'a> CategoryService for CategoryServiceImpl<'a> {
    async fn upsert_category(&self, category: Category) -> Result<Category, CategoryServiceError> {
        self.dynamodb_repository
            .put_category_record(category.clone().try_into()?)
            .await?;
        self.opensearch_search
            .index_category_document(category.clone().try_into()?)
            .await?;

        Ok(category)
    }

    async fn find_category(
        &self,
        category_id: &CategoryId,
    ) -> Result<Category, CategoryServiceError> {
        self.dynamodb_repository
            .get_category_record(category_id)
            .await?
            .map(Category::from)
            .ok_or_else(|| CategoryServiceError::CategoryNotExists(category_id.clone()))
    }

    async fn view_category(
        &self,
        category_id: &CategoryId,
        languages: &[Language],
    ) -> Result<LocalizedCategory, CategoryServiceError> {
        let category = self.find_category(category_id).await?;
        Ok(category.localized(languages))
    }

    async fn find_similar(
        &self,
        embedding: &[f32],
        k: u16,
    ) -> Result<Vec<(Category, f64)>, opensearch::Error> {
        let search_res = self.opensearch_search.exact_k_nn(embedding, k).await?;
        let similar = search_res
            .hits
            .hits
            .into_iter()
            .map(|hit| (hit.source.into(), hit.score.unwrap_or(0.0)))
            .collect();

        Ok(similar)
    }

    async fn search_categories(
        &self,
        search: &CategorySearch,
        sort: &Option<Sort<SortCategoryField>>,
    ) -> Result<Vec<LocalizedCategory>, CategoryServiceError> {
        let sort = (*sort).unwrap_or(Sort {
            sort: SortCategoryField::Score,
            order: SortOrder::Desc,
        });
        let sort = if search.name_query.is_none() && matches!(sort.sort, SortCategoryField::Score) {
            Sort {
                sort: SortCategoryField::Name,
                order: SortOrder::Asc,
            }
        } else {
            sort
        };

        let search_response = self
            .opensearch_search
            .search_category_documents(search, &sort)
            .await?;

        let categories = search_response
            .hits
            .hits
            .into_iter()
            .map(|hit| Category::from(hit.source).localized(&[search.language]))
            .collect();

        Ok(categories)
    }

    async fn find_categories(&self) -> Result<Vec<Category>, CategoryServiceError> {
        let records = self.dynamodb_repository.query_category_records().await?;
        let categories = records.into_iter().map(Category::from).collect();
        Ok(categories)
    }

    async fn view_categories(
        &self,
        languages: &[Language],
    ) -> Result<Vec<LocalizedCategory>, CategoryServiceError> {
        let categories = self.find_categories().await?;
        let localized = categories
            .into_iter()
            .map(|c| c.localized(languages))
            .collect();
        Ok(localized)
    }
}

#[cfg(test)]
mod tests {
    use super::{CategoryService, CategoryServiceError, CategoryServiceImpl};
    use crate::category::{
        core::Category, document::CategoryDocument,
        dynamodb_repository::MockCategoryDynamoDbRepository,
        opensearch_repository::MockCategoryOpenSearchRepository,
    };
    use aws_sdk_dynamodb::{
        config::http::HttpResponse,
        error::{ConnectorError, SdkError},
        operation::{
            get_item::GetItemError,
            put_item::{PutItemError, PutItemOutput},
            query::QueryError,
        },
    };
    use common::{
        category_key::CategoryId,
        language::domain::Language,
        opensearch::{
            index_response::IndexResponse,
            search_response::{HitsMetadata, SearchHit, SearchResponse, ShardStats, TotalHits},
        },
    };
    use fake::{Fake, Faker};
    use rstest;
    use serde::ser::Error;

    fn mk_index_response() -> IndexResponse {
        serde_json::from_str(
            r#"{
                "_index": "categories",
                "_id": "category-key",
                "_version": 1,
                "result": "created",
                "_shards": { "total": 1, "successful": 1, "failed": 0 },
                "_seq_no": 1,
                "_primary_term": 1
            }"#,
        )
        .expect("should deserialize index response")
    }

    mod upsert_category {
        use super::*;

        #[tokio::test]
        async fn should_upsert_category_when_repositories_succeed_for_category_service() {
            let category: Category = Faker.fake();
            let expected_category_id = category.category_id.clone();
            let expected_category_id_for_index = category.category_id.clone();
            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_put_category_record()
                .withf(move |record| record.category_id == expected_category_id)
                .once()
                .returning(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let mut opensearch_repository = MockCategoryOpenSearchRepository::default();
            opensearch_repository
                .expect_index_category_document()
                .withf(move |document| document.category_id == expected_category_id_for_index)
                .once()
                .returning(|_| Box::pin(async { Ok(mk_index_response()) }));

            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.upsert_category(category.clone()).await.unwrap();

            assert_eq!(actual, category);
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
            PutItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_dynamodb_error_when_put_fails_for_category_service(
            #[case] expected: SdkError<PutItemError, HttpResponse>,
        ) {
            let category: Category = Faker.fake();
            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_put_category_record()
                .once()
                .return_once(|_| Box::pin(async { Err(expected) }));

            let mut opensearch_repository = MockCategoryOpenSearchRepository::default();
            opensearch_repository
                .expect_index_category_document()
                .never();

            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.upsert_category(category).await;

            assert!(matches!(
                actual.unwrap_err(),
                CategoryServiceError::DynamoDbSdkPutItemError(_)
            ));
        }

        #[tokio::test]
        async fn should_propagate_opensearch_error_when_index_fails_for_category_service() {
            let category: Category = Faker.fake();
            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_put_category_record()
                .once()
                .returning(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let mut opensearch_repository = MockCategoryOpenSearchRepository::default();
            opensearch_repository
                .expect_index_category_document()
                .once()
                .returning(|_| {
                    Box::pin(async {
                        Err(opensearch::Error::from(serde_json::Error::custom(
                            "Something went wrong",
                        )))
                    })
                });

            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.upsert_category(category).await;

            assert!(matches!(
                actual.unwrap_err(),
                CategoryServiceError::OpenSearchError(_)
            ));
        }
    }

    mod find_category {
        use super::*;

        #[tokio::test]
        async fn should_return_category_when_exists_for_category_service() {
            let category: Category = Faker.fake();
            let category_id = category.category_id.clone();
            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_get_category_record()
                .once()
                .return_once(move |_| {
                    Box::pin(async move { Ok(Some(category.try_into().unwrap())) })
                });

            let opensearch_repository = MockCategoryOpenSearchRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_category(&category_id).await.unwrap();

            assert_eq!(actual.category_id, category_id);
        }

        #[tokio::test]
        async fn should_err_when_category_missing_for_category_service() {
            let category_id = CategoryId::from("missing-category");
            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_get_category_record()
                .once()
                .return_once(|_| Box::pin(async { Ok(None) }));

            let opensearch_repository = MockCategoryOpenSearchRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_category(&category_id).await;

            assert!(matches!(
                actual.unwrap_err(),
                CategoryServiceError::CategoryNotExists(err_id) if err_id == category_id
            ));
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
            GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_dynamodb_error_when_get_fails_for_category_service(
            #[case] expected: SdkError<GetItemError, HttpResponse>,
        ) {
            let category_id = CategoryId::from("missing-category");
            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_get_category_record()
                .once()
                .return_once(move |_| Box::pin(async { Err(expected) }));

            let opensearch_repository = MockCategoryOpenSearchRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_category(&category_id).await;

            assert!(matches!(
                actual.unwrap_err(),
                CategoryServiceError::DynamoDbSdkGetItemError(_)
            ));
        }
    }

    mod find_similar {
        use super::*;

        #[tokio::test]
        async fn should_return_similar_categories_when_opensearch_succeeds_for_category_service() {
            let category: Category = Faker.fake();
            let mut other_category: Category = Faker.fake();
            loop {
                if other_category.category_id != category.category_id {
                    break;
                }
                other_category = Faker.fake();
            }

            let category_document: CategoryDocument = category.clone().try_into().unwrap();
            let other_category_document: CategoryDocument =
                other_category.clone().try_into().unwrap();
            let mut opensearch_repository = MockCategoryOpenSearchRepository::default();
            opensearch_repository
                .expect_exact_k_nn()
                .once()
                .return_once(move |_, _| {
                    let response = SearchResponse {
                        took: 12,
                        timed_out: false,
                        shards: ShardStats {
                            total: 1,
                            successful: 1,
                            skipped: 0,
                            failed: 0,
                        },
                        hits: HitsMetadata {
                            total: TotalHits {
                                value: 2,
                                relation: "eq".to_string(),
                            },
                            max_score: Some(0.42),
                            hits: vec![
                                SearchHit {
                                    index: "categories".to_string(),
                                    id: category_document.category_id.to_string(),
                                    score: Some(0.42),
                                    source: category_document,
                                    sort: None,
                                },
                                SearchHit {
                                    index: "categories".to_string(),
                                    id: other_category_document.category_id.to_string(),
                                    score: None,
                                    source: other_category_document,
                                    sort: None,
                                },
                            ],
                        },
                    };

                    Box::pin(async { Ok(response) })
                });

            let dynamodb_repository = MockCategoryDynamoDbRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_similar(&[0.1, 0.2], 2).await.unwrap();

            assert_eq!(actual.len(), 2);
            assert_eq!(actual[0].0.category_id, category.category_id);
            assert_eq!(actual[0].1, 0.42);
            assert_eq!(actual[1].0.category_id, other_category.category_id);
            assert_eq!(actual[1].1, 0.0);
        }

        #[tokio::test]
        async fn should_propagate_opensearch_error_when_search_fails_for_category_service() {
            let mut opensearch_repository = MockCategoryOpenSearchRepository::default();
            opensearch_repository
                .expect_exact_k_nn()
                .once()
                .return_once(|_, _| {
                    Box::pin(async {
                        Err(opensearch::Error::from(serde_json::Error::custom(
                            "Something went wrong",
                        )))
                    })
                });

            let dynamodb_repository = MockCategoryDynamoDbRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_similar(&[0.1, 0.2], 1).await;

            assert!(actual.is_err());
        }
    }

    mod view_category {
        use super::*;

        #[tokio::test]
        async fn should_return_localized_category_when_exists_for_category_service() {
            let category: Category = Faker.fake();
            let category_id = category.category_id.clone();
            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_get_category_record()
                .once()
                .return_once(move |_| {
                    Box::pin(async move { Ok(Some(category.try_into().unwrap())) })
                });

            let opensearch_repository = MockCategoryOpenSearchRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service
                .view_category(&category_id, &[Language::En])
                .await
                .unwrap();

            assert_eq!(actual.category_id, category_id);
            assert!(actual.display_name.is_some());
        }

        #[tokio::test]
        async fn should_err_when_category_missing_for_view_category() {
            let category_id = CategoryId::from("missing-category");
            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_get_category_record()
                .once()
                .return_once(|_| Box::pin(async { Ok(None) }));

            let opensearch_repository = MockCategoryOpenSearchRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.view_category(&category_id, &[Language::En]).await;

            assert!(matches!(
                actual.unwrap_err(),
                CategoryServiceError::CategoryNotExists(err_id) if err_id == category_id
            ));
        }
    }

    mod search_categories {
        use super::*;
        use crate::category::category_search::CategorySearch;

        #[tokio::test]
        async fn should_return_localized_categories_when_opensearch_succeeds_for_category_service()
        {
            let category: Category = Faker.fake();
            let category_document: CategoryDocument = category.clone().try_into().unwrap();
            let expected_category_id = category.category_id.clone();

            let mut opensearch_repository = MockCategoryOpenSearchRepository::default();
            opensearch_repository
                .expect_search_category_documents()
                .once()
                .return_once(move |_, _| {
                    let response = SearchResponse {
                        took: 12,
                        timed_out: false,
                        shards: ShardStats {
                            total: 1,
                            successful: 1,
                            skipped: 0,
                            failed: 0,
                        },
                        hits: HitsMetadata {
                            total: TotalHits {
                                value: 1,
                                relation: "eq".to_string(),
                            },
                            max_score: Some(1.0),
                            hits: vec![SearchHit {
                                index: "categories".to_string(),
                                id: category_document.category_id.to_string(),
                                score: Some(1.0),
                                source: category_document,
                                sort: None,
                            }],
                        },
                    };

                    Box::pin(async { Ok(response) })
                });

            let dynamodb_repository = MockCategoryDynamoDbRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let search = CategorySearch {
                language: Language::En,
                name_query: Some("test".try_into().unwrap()),
            };
            let actual = service.search_categories(&search, &None).await.unwrap();

            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].category_id, expected_category_id);
            assert!(actual[0].display_name.is_some());
        }

        #[tokio::test]
        async fn should_propagate_opensearch_error_when_search_fails_for_search_categories() {
            let mut opensearch_repository = MockCategoryOpenSearchRepository::default();
            opensearch_repository
                .expect_search_category_documents()
                .once()
                .return_once(|_, _| {
                    Box::pin(async {
                        Err(opensearch::Error::from(serde_json::Error::custom(
                            "Something went wrong",
                        )))
                    })
                });

            let dynamodb_repository = MockCategoryDynamoDbRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let search = CategorySearch {
                language: Language::En,
                name_query: Some("test".try_into().unwrap()),
            };
            let actual = service.search_categories(&search, &None).await;

            assert!(matches!(
                actual.unwrap_err(),
                CategoryServiceError::OpenSearchError(_)
            ));
        }

        #[tokio::test]
        async fn should_search_via_opensearch_when_empty_search_for_category_service() {
            let category: Category = Faker.fake();
            let category_document: CategoryDocument = category.clone().try_into().unwrap();
            let expected_category_id = category.category_id.clone();

            let mut opensearch_repository = MockCategoryOpenSearchRepository::default();
            opensearch_repository
                .expect_search_category_documents()
                .once()
                .return_once(move |_, _| {
                    let response = SearchResponse {
                        took: 12,
                        timed_out: false,
                        shards: ShardStats {
                            total: 1,
                            successful: 1,
                            skipped: 0,
                            failed: 0,
                        },
                        hits: HitsMetadata {
                            total: TotalHits {
                                value: 1,
                                relation: "eq".to_string(),
                            },
                            max_score: Some(1.0),
                            hits: vec![SearchHit {
                                index: "categories".to_string(),
                                id: category_document.category_id.to_string(),
                                score: Some(1.0),
                                source: category_document,
                                sort: None,
                            }],
                        },
                    };

                    Box::pin(async { Ok(response) })
                });

            let dynamodb_repository = MockCategoryDynamoDbRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let search = CategorySearch {
                language: Language::En,
                name_query: None,
            };
            let actual = service.search_categories(&search, &None).await.unwrap();

            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].category_id, expected_category_id);
        }
    }

    mod find_categories {
        use super::*;

        #[tokio::test]
        async fn should_return_categories_when_dynamodb_succeeds_for_category_service() {
            let category: Category = Faker.fake();
            let expected_category_id = category.category_id.clone();
            let record = category.try_into().unwrap();

            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_query_category_records()
                .once()
                .return_once(move || Box::pin(async move { Ok(vec![record]) }));

            let opensearch_repository = MockCategoryOpenSearchRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_categories().await.unwrap();

            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].category_id, expected_category_id);
        }

        #[tokio::test]
        async fn should_return_empty_when_no_categories_exist_for_category_service() {
            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_query_category_records()
                .once()
                .return_once(|| Box::pin(async { Ok(vec![]) }));

            let opensearch_repository = MockCategoryOpenSearchRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_categories().await.unwrap();

            assert!(actual.is_empty());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[trace]
        async fn should_propagate_dynamodb_error_when_query_fails_for_category_service(
            #[case] expected: SdkError<QueryError, HttpResponse>,
        ) {
            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_query_category_records()
                .once()
                .return_once(move || Box::pin(async { Err(expected) }));

            let opensearch_repository = MockCategoryOpenSearchRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_categories().await;

            assert!(matches!(
                actual.unwrap_err(),
                CategoryServiceError::DynamoDbSdkQueryError(_)
            ));
        }
    }

    mod view_categories {
        use super::*;

        #[tokio::test]
        async fn should_return_localized_categories_when_dynamodb_succeeds_for_category_service() {
            let category: Category = Faker.fake();
            let expected_category_id = category.category_id.clone();
            let record = category.try_into().unwrap();

            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_query_category_records()
                .once()
                .return_once(move || Box::pin(async move { Ok(vec![record]) }));

            let opensearch_repository = MockCategoryOpenSearchRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.view_categories(&[Language::En]).await.unwrap();

            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].category_id, expected_category_id);
            assert!(actual[0].display_name.is_some());
        }

        #[tokio::test]
        async fn should_return_empty_when_no_categories_exist_for_view_categories() {
            let mut dynamodb_repository = MockCategoryDynamoDbRepository::default();
            dynamodb_repository
                .expect_query_category_records()
                .once()
                .return_once(|| Box::pin(async { Ok(vec![]) }));

            let opensearch_repository = MockCategoryOpenSearchRepository::default();
            let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.view_categories(&[Language::En]).await.unwrap();

            assert!(actual.is_empty());
        }
    }
}
