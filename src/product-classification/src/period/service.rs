use crate::period::{
    core::{LocalizedPeriod, Period},
    dynamodb_repository::PeriodDynamoDbRepository,
    opensearch_repository::PeriodOpenSearchRepository,
    period_search::PeriodSearch,
    sort_period_field::SortPeriodField,
};
use aws_sdk_dynamodb::{
    error::SdkError,
    operation::{get_item::GetItemError, put_item::PutItemError, query::QueryError},
};
use common::{
    error::missing_field::MissingRequiredField,
    language::domain::Language,
    period_key::PeriodId,
    sort::{Sort, SortOrder},
};

#[derive(Debug, thiserror::Error)]
pub enum PeriodServiceError {
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),

    #[error("Period '{0}' does not exist")]
    PeriodNotExists(PeriodId),

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
    use super::PeriodServiceError;
    use common::api::error::ApiError;
    use common::api::error_code::{INTERNAL_SERVER_ERROR, NOT_FOUND};

    impl From<PeriodServiceError> for ApiError {
        fn from(err: PeriodServiceError) -> Self {
            match err {
                PeriodServiceError::PeriodNotExists(_) => {
                    ApiError::not_found(NOT_FOUND, Box::new(err))
                }
                PeriodServiceError::OpenSearchError(e) => e.into(),
                PeriodServiceError::DynamoDbSdkPutItemError(e) => e.into(),
                PeriodServiceError::DynamoDbSdkGetItemError(e) => e.into(),
                PeriodServiceError::DynamoDbSdkQueryError(e) => e.into(),
                PeriodServiceError::MappingError(e) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(e))
                }
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait PeriodService {
    async fn upsert_period(&self, period: Period) -> Result<Period, PeriodServiceError>;

    async fn find_period(&self, period_id: &PeriodId) -> Result<Period, PeriodServiceError>;

    async fn view_period(
        &self,
        period_id: &PeriodId,
        languages: &[Language],
    ) -> Result<LocalizedPeriod, PeriodServiceError>;

    async fn find_similar(
        &self,
        embedding: &[f32],
        k: u16,
    ) -> Result<Vec<(Period, f64)>, opensearch::Error>;

    async fn search_periods(
        &self,
        search: &PeriodSearch,
        sort: &Option<Sort<SortPeriodField>>,
    ) -> Result<Vec<LocalizedPeriod>, PeriodServiceError>;

    async fn find_periods(&self) -> Result<Vec<Period>, PeriodServiceError>;

    async fn view_periods(
        &self,
        languages: &[Language],
    ) -> Result<Vec<LocalizedPeriod>, PeriodServiceError>;
}

pub struct PeriodServiceImpl<'a> {
    pub dynamodb_repository: &'a (dyn PeriodDynamoDbRepository + Send + Sync),
    pub opensearch_search: &'a (dyn PeriodOpenSearchRepository + Send + Sync),
}

impl<'a> PeriodServiceImpl<'a> {
    pub fn new(
        dynamodb_repository: &'a (dyn PeriodDynamoDbRepository + Send + Sync),
        opensearch_search: &'a (dyn PeriodOpenSearchRepository + Send + Sync),
    ) -> Self {
        Self {
            dynamodb_repository,
            opensearch_search,
        }
    }
}

#[async_trait::async_trait]
impl<'a> PeriodService for PeriodServiceImpl<'a> {
    async fn upsert_period(&self, period: Period) -> Result<Period, PeriodServiceError> {
        self.dynamodb_repository
            .put_period_record(period.clone().try_into()?)
            .await?;
        self.opensearch_search
            .index_period_document(period.clone().try_into()?)
            .await?;

        Ok(period)
    }

    async fn find_period(&self, period_id: &PeriodId) -> Result<Period, PeriodServiceError> {
        self.dynamodb_repository
            .get_period_record(period_id)
            .await?
            .map(Period::from)
            .ok_or_else(|| PeriodServiceError::PeriodNotExists(period_id.clone()))
    }

    async fn view_period(
        &self,
        period_id: &PeriodId,
        languages: &[Language],
    ) -> Result<LocalizedPeriod, PeriodServiceError> {
        let period = self.find_period(period_id).await?;
        Ok(period.localized(languages))
    }

    async fn find_similar(
        &self,
        embedding: &[f32],
        k: u16,
    ) -> Result<Vec<(Period, f64)>, opensearch::Error> {
        let search_res = self.opensearch_search.exact_k_nn(embedding, k).await?;
        let similar = search_res
            .hits
            .hits
            .into_iter()
            .map(|hit| (hit.source.into(), hit.score.unwrap_or(0.0)))
            .collect();

        Ok(similar)
    }

    async fn search_periods(
        &self,
        search: &PeriodSearch,
        sort: &Option<Sort<SortPeriodField>>,
    ) -> Result<Vec<LocalizedPeriod>, PeriodServiceError> {
        let sort = (*sort).unwrap_or(Sort {
            sort: SortPeriodField::Score,
            order: SortOrder::Desc,
        });
        let sort = if search.name_query.is_none() && matches!(sort.sort, SortPeriodField::Score) {
            Sort {
                sort: SortPeriodField::Name,
                order: SortOrder::Asc,
            }
        } else {
            sort
        };

        let search_response = self
            .opensearch_search
            .search_period_documents(search, &sort)
            .await?;

        let periods = search_response
            .hits
            .hits
            .into_iter()
            .map(|hit| Period::from(hit.source).localized(&[search.language]))
            .collect();

        Ok(periods)
    }

    async fn find_periods(&self) -> Result<Vec<Period>, PeriodServiceError> {
        let records = self.dynamodb_repository.query_period_records().await?;
        let periods = records.into_iter().map(Period::from).collect();
        Ok(periods)
    }

    async fn view_periods(
        &self,
        languages: &[Language],
    ) -> Result<Vec<LocalizedPeriod>, PeriodServiceError> {
        let periods = self.find_periods().await?;
        let localized = periods
            .into_iter()
            .map(|p| p.localized(languages))
            .collect();
        Ok(localized)
    }
}

#[cfg(test)]
mod tests {
    use super::{PeriodService, PeriodServiceError, PeriodServiceImpl};
    use crate::period::{
        core::Period, document::PeriodDocument, dynamodb_repository::MockPeriodDynamoDbRepository,
        opensearch_repository::MockPeriodOpenSearchRepository,
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
        language::domain::Language,
        opensearch::{
            index_response::IndexResponse,
            search_response::{HitsMetadata, SearchHit, SearchResponse, ShardStats, TotalHits},
        },
        period_key::PeriodId,
    };
    use fake::{Fake, Faker};
    use rstest;
    use serde::ser::Error;

    fn mk_index_response() -> IndexResponse {
        serde_json::from_str(
            r#"{
                "_index": "periods",
                "_id": "period-key",
                "_version": 1,
                "result": "created",
                "_shards": { "total": 1, "successful": 1, "failed": 0 },
                "_seq_no": 1,
                "_primary_term": 1
            }"#,
        )
        .expect("should deserialize index response")
    }

    mod upsert_period {
        use super::*;

        #[tokio::test]
        async fn should_upsert_period_when_repositories_succeed_for_period_service() {
            let period: Period = Faker.fake();
            let expected_period_id = period.period_id.clone();
            let expected_period_id_for_index = period.period_id.clone();
            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_put_period_record()
                .withf(move |record| record.period_id == expected_period_id)
                .once()
                .returning(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let mut opensearch_repository = MockPeriodOpenSearchRepository::default();
            opensearch_repository
                .expect_index_period_document()
                .withf(move |document| document.period_id == expected_period_id_for_index)
                .once()
                .returning(|_| Box::pin(async { Ok(mk_index_response()) }));

            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.upsert_period(period.clone()).await.unwrap();

            assert_eq!(actual, period);
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
        async fn should_propagate_dynamodb_error_when_put_fails_for_period_service(
            #[case] expected: SdkError<PutItemError, HttpResponse>,
        ) {
            let period: Period = Faker.fake();
            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_put_period_record()
                .once()
                .return_once(|_| Box::pin(async { Err(expected) }));

            let mut opensearch_repository = MockPeriodOpenSearchRepository::default();
            opensearch_repository.expect_index_period_document().never();

            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.upsert_period(period).await;

            assert!(matches!(
                actual.unwrap_err(),
                PeriodServiceError::DynamoDbSdkPutItemError(_)
            ));
        }

        #[tokio::test]
        async fn should_propagate_opensearch_error_when_index_fails_for_period_service() {
            let period: Period = Faker.fake();
            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_put_period_record()
                .once()
                .returning(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let mut opensearch_repository = MockPeriodOpenSearchRepository::default();
            opensearch_repository
                .expect_index_period_document()
                .once()
                .returning(|_| {
                    Box::pin(async {
                        Err(opensearch::Error::from(serde_json::Error::custom(
                            "Something went wrong",
                        )))
                    })
                });

            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.upsert_period(period).await;

            assert!(matches!(
                actual.unwrap_err(),
                PeriodServiceError::OpenSearchError(_)
            ));
        }
    }

    mod find_period {
        use super::*;

        #[tokio::test]
        async fn should_return_period_when_exists_for_period_service() {
            let period: Period = Faker.fake();
            let period_id = period.period_id.clone();
            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_get_period_record()
                .once()
                .return_once(move |_| {
                    Box::pin(async move { Ok(Some(period.try_into().unwrap())) })
                });

            let opensearch_repository = MockPeriodOpenSearchRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_period(&period_id).await.unwrap();

            assert_eq!(actual.period_id, period_id);
        }

        #[tokio::test]
        async fn should_err_when_period_missing_for_period_service() {
            let period_id = PeriodId::from("missing-period");
            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_get_period_record()
                .once()
                .return_once(|_| Box::pin(async { Ok(None) }));

            let opensearch_repository = MockPeriodOpenSearchRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_period(&period_id).await;

            assert!(matches!(
                actual.unwrap_err(),
                PeriodServiceError::PeriodNotExists(err_id) if err_id == period_id
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
        async fn should_propagate_dynamodb_error_when_get_fails_for_period_service(
            #[case] expected: SdkError<GetItemError, HttpResponse>,
        ) {
            let period_id = PeriodId::from("missing-period");
            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_get_period_record()
                .once()
                .return_once(move |_| Box::pin(async { Err(expected) }));

            let opensearch_repository = MockPeriodOpenSearchRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_period(&period_id).await;

            assert!(matches!(
                actual.unwrap_err(),
                PeriodServiceError::DynamoDbSdkGetItemError(_)
            ));
        }
    }

    mod find_similar {
        use super::*;

        #[tokio::test]
        async fn should_return_similar_periods_when_opensearch_succeeds_for_period_service() {
            let period: Period = Faker.fake();
            let mut other_period: Period = Faker.fake();
            loop {
                if other_period.period_id != period.period_id {
                    break;
                }
                other_period = Faker.fake();
            }

            let period_document: PeriodDocument = period.clone().try_into().unwrap();
            let other_period_document: PeriodDocument = other_period.clone().try_into().unwrap();
            let mut opensearch_repository = MockPeriodOpenSearchRepository::default();
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
                                    index: "periods".to_string(),
                                    id: period_document.period_id.to_string(),
                                    score: Some(0.42),
                                    source: period_document,
                                    sort: None,
                                },
                                SearchHit {
                                    index: "periods".to_string(),
                                    id: other_period_document.period_id.to_string(),
                                    score: None,
                                    source: other_period_document,
                                    sort: None,
                                },
                            ],
                        },
                    };

                    Box::pin(async { Ok(response) })
                });

            let dynamodb_repository = MockPeriodDynamoDbRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_similar(&[0.1, 0.2], 2).await.unwrap();

            assert_eq!(actual.len(), 2);
            assert_eq!(actual[0].0.period_id, period.period_id);
            assert_eq!(actual[0].1, 0.42);
            assert_eq!(actual[1].0.period_id, other_period.period_id);
            assert_eq!(actual[1].1, 0.0);
        }

        #[tokio::test]
        async fn should_propagate_opensearch_error_when_search_fails_for_period_service() {
            let mut opensearch_repository = MockPeriodOpenSearchRepository::default();
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

            let dynamodb_repository = MockPeriodDynamoDbRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_similar(&[0.1, 0.2], 1).await;

            assert!(actual.is_err());
        }
    }

    mod view_period {
        use super::*;

        #[tokio::test]
        async fn should_return_localized_period_when_exists_for_period_service() {
            let period: Period = Faker.fake();
            let period_id = period.period_id.clone();
            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_get_period_record()
                .once()
                .return_once(move |_| {
                    Box::pin(async move { Ok(Some(period.try_into().unwrap())) })
                });

            let opensearch_repository = MockPeriodOpenSearchRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service
                .view_period(&period_id, &[Language::En])
                .await
                .unwrap();

            assert_eq!(actual.period_id, period_id);
        }

        #[tokio::test]
        async fn should_err_when_period_missing_for_view_period() {
            let period_id = PeriodId::from("missing-period");
            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_get_period_record()
                .once()
                .return_once(|_| Box::pin(async { Ok(None) }));

            let opensearch_repository = MockPeriodOpenSearchRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.view_period(&period_id, &[Language::En]).await;

            assert!(matches!(
                actual.unwrap_err(),
                PeriodServiceError::PeriodNotExists(err_id) if err_id == period_id
            ));
        }
    }

    mod search_periods {
        use super::*;
        use crate::period::period_search::PeriodSearch;

        #[tokio::test]
        async fn should_return_localized_periods_when_opensearch_succeeds_for_period_service() {
            let period: Period = Faker.fake();
            let period_document: PeriodDocument = period.clone().try_into().unwrap();
            let expected_period_id = period.period_id.clone();

            let mut opensearch_repository = MockPeriodOpenSearchRepository::default();
            opensearch_repository
                .expect_search_period_documents()
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
                                index: "periods".to_string(),
                                id: period_document.period_id.to_string(),
                                score: Some(1.0),
                                source: period_document,
                                sort: None,
                            }],
                        },
                    };

                    Box::pin(async { Ok(response) })
                });

            let dynamodb_repository = MockPeriodDynamoDbRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let search = PeriodSearch {
                language: Language::En,
                name_query: Some("test".try_into().unwrap()),
            };
            let actual = service.search_periods(&search, &None).await.unwrap();

            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].period_id, expected_period_id);
        }

        #[tokio::test]
        async fn should_propagate_opensearch_error_when_search_fails_for_search_periods() {
            let mut opensearch_repository = MockPeriodOpenSearchRepository::default();
            opensearch_repository
                .expect_search_period_documents()
                .once()
                .return_once(|_, _| {
                    Box::pin(async {
                        Err(opensearch::Error::from(serde_json::Error::custom(
                            "Something went wrong",
                        )))
                    })
                });

            let dynamodb_repository = MockPeriodDynamoDbRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let search = PeriodSearch {
                language: Language::En,
                name_query: Some("test".try_into().unwrap()),
            };
            let actual = service.search_periods(&search, &None).await;

            assert!(matches!(
                actual.unwrap_err(),
                PeriodServiceError::OpenSearchError(_)
            ));
        }

        #[tokio::test]
        async fn should_search_via_opensearch_when_empty_search_for_period_service() {
            let period: Period = Faker.fake();
            let period_document: PeriodDocument = period.clone().try_into().unwrap();
            let expected_period_id = period.period_id.clone();

            let mut opensearch_repository = MockPeriodOpenSearchRepository::default();
            opensearch_repository
                .expect_search_period_documents()
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
                                index: "periods".to_string(),
                                id: period_document.period_id.to_string(),
                                score: Some(1.0),
                                source: period_document,
                                sort: None,
                            }],
                        },
                    };

                    Box::pin(async { Ok(response) })
                });

            let dynamodb_repository = MockPeriodDynamoDbRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let search = PeriodSearch {
                language: Language::En,
                name_query: None,
            };
            let actual = service.search_periods(&search, &None).await.unwrap();

            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].period_id, expected_period_id);
        }
    }

    mod find_periods {
        use super::*;

        #[tokio::test]
        async fn should_return_periods_when_dynamodb_succeeds_for_period_service() {
            let period: Period = Faker.fake();
            let expected_period_id = period.period_id.clone();
            let record = period.try_into().unwrap();

            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_query_period_records()
                .once()
                .return_once(move || Box::pin(async move { Ok(vec![record]) }));

            let opensearch_repository = MockPeriodOpenSearchRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_periods().await.unwrap();

            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].period_id, expected_period_id);
        }

        #[tokio::test]
        async fn should_return_empty_when_no_periods_exist_for_period_service() {
            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_query_period_records()
                .once()
                .return_once(|| Box::pin(async { Ok(vec![]) }));

            let opensearch_repository = MockPeriodOpenSearchRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_periods().await.unwrap();

            assert!(actual.is_empty());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[trace]
        async fn should_propagate_dynamodb_error_when_query_fails_for_period_service(
            #[case] expected: SdkError<QueryError, HttpResponse>,
        ) {
            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_query_period_records()
                .once()
                .return_once(move || Box::pin(async { Err(expected) }));

            let opensearch_repository = MockPeriodOpenSearchRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.find_periods().await;

            assert!(matches!(
                actual.unwrap_err(),
                PeriodServiceError::DynamoDbSdkQueryError(_)
            ));
        }
    }

    mod view_periods {
        use super::*;

        #[tokio::test]
        async fn should_return_localized_periods_when_dynamodb_succeeds_for_period_service() {
            let period: Period = Faker.fake();
            let expected_period_id = period.period_id.clone();
            let record = period.try_into().unwrap();

            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_query_period_records()
                .once()
                .return_once(move || Box::pin(async move { Ok(vec![record]) }));

            let opensearch_repository = MockPeriodOpenSearchRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.view_periods(&[Language::En]).await.unwrap();

            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].period_id, expected_period_id);
        }

        #[tokio::test]
        async fn should_return_empty_when_no_periods_exist_for_view_periods() {
            let mut dynamodb_repository = MockPeriodDynamoDbRepository::default();
            dynamodb_repository
                .expect_query_period_records()
                .once()
                .return_once(|| Box::pin(async { Ok(vec![]) }));

            let opensearch_repository = MockPeriodOpenSearchRepository::default();
            let service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

            let actual = service.view_periods(&[Language::En]).await.unwrap();

            assert!(actual.is_empty());
        }
    }
}
