use aws_sdk_dynamodb::{config::http::HttpResponse, error::SdkError};
use common::{sort::SortOrder, user_id::UserId};
use search_filter_core::user_search_filter::UserSearchFilter;
use search_filter_core::{search_filter::SearchFilter, search_filter_id::SearchFilterId};
use search_filter_dynamodb::repository::SearchFilterDynamoDbRepository;
use time::OffsetDateTime;

#[derive(thiserror::Error, Debug)]
pub enum SearchFilterError {
    #[error("SearchFilter with UserId '{0}' and SearchFilterId '{1}' not found.")]
    SearchFilterNotFound(UserId, SearchFilterId),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for QueryItem: {0}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>),

    #[error("Encountered DynamoDB SdkError for PutItem: {0}")]
    SdkPutItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for DeleteItem: {0}")]
    SdkDeleteItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::delete_item::DeleteItemError, HttpResponse>,
    ),
}

#[cfg(feature = "api")]
pub mod api {
    use crate::service::SearchFilterError;
    use common::api::error::ApiError;
    use common::api::error_code::SEARCH_FILTER_NOT_FOUND;
    use tracing::error;

    impl From<SearchFilterError> for ApiError {
        fn from(err: SearchFilterError) -> Self {
            match err {
                SearchFilterError::SearchFilterNotFound(_, _) => {
                    ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                }
                SearchFilterError::SdkGetItemError(err) => {
                    error!(error = ?err, "Encountered SdkGetItemError while getting search-filter.");
                    err.into()
                }
                SearchFilterError::SdkQueryError(err) => {
                    error!(error = ?err, "Encountered SdkQueryError while querying search-filters.");
                    err.into()
                }
                SearchFilterError::SdkPutItemError(err) => {
                    error!(error = ?err, "Encountered SdkPutItemError while saving search-filter.");
                    err.into()
                }
                SearchFilterError::SdkDeleteItemError(err) => {
                    error!(error = ?err, "Encountered SdkDeleteItemError while deleting search-filter.");
                    err.into()
                }
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait SearchFilterService {
    async fn find_search_filters(
        &self,
        user_id: &UserId,
        sort_by_created: &Option<SortOrder>,
    ) -> Result<Vec<UserSearchFilter>, SearchFilterError>;

    async fn find_search_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<UserSearchFilter, SearchFilterError>;

    async fn save_search_filter(
        &self,
        user_id: &UserId,
        search_filter: SearchFilter,
    ) -> Result<UserSearchFilter, SearchFilterError>;

    async fn delete_search_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<(), SearchFilterError>;
}

pub struct SearchFilterServiceImpl<'a> {
    repository: &'a (dyn SearchFilterDynamoDbRepository + Sync),
}

impl<'a> SearchFilterServiceImpl<'a> {
    pub fn new(repository: &'a (dyn SearchFilterDynamoDbRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl<'a> SearchFilterService for SearchFilterServiceImpl<'a> {
    async fn find_search_filters(
        &self,
        user_id: &UserId,
        sort_by_created: &Option<SortOrder>,
    ) -> Result<Vec<UserSearchFilter>, SearchFilterError> {
        let search_filters = self
            .repository
            .query_search_filter_records(
                user_id,
                matches!(sort_by_created.unwrap_or(SortOrder::Asc), SortOrder::Asc),
            )
            .await?
            .into_iter()
            .map(UserSearchFilter::from)
            .collect();
        Ok(search_filters)
    }

    async fn find_search_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<UserSearchFilter, SearchFilterError> {
        let search_filter = self
            .repository
            .get_search_filter_record(user_id, search_filter_id)
            .await?
            .map(UserSearchFilter::from)
            .ok_or_else(|| SearchFilterError::SearchFilterNotFound(*user_id, *search_filter_id))?;
        Ok(search_filter)
    }

    async fn save_search_filter(
        &self,
        user_id: &UserId,
        search_filter: SearchFilter,
    ) -> Result<UserSearchFilter, SearchFilterError> {
        let user_search_filter = UserSearchFilter {
            user_id: *user_id,
            search_filter_id: SearchFilterId::new(),
            search_filter,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };

        let _ = self
            .repository
            .put_search_filter_record(user_search_filter.clone().into())
            .await?;

        Ok(user_search_filter)
    }

    async fn delete_search_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<(), SearchFilterError> {
        // exists guard
        let _ = self.find_search_filter(user_id, search_filter_id).await?;
        let _ = self
            .repository
            .delete_search_filter_record(user_id, search_filter_id)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    mod find_search_filters {
        use crate::service::{SearchFilterError, SearchFilterService, SearchFilterServiceImpl};
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::sort::SortOrder;
        use common::user_id::UserId;
        use search_filter_dynamodb::repository::MockSearchFilterDynamoDbRepository;
        use search_filter_dynamodb::search_filter_record::SearchFilterRecord;

        #[rstest::rstest]
        #[case::empty(0)]
        #[case::non_empty(42)]
        #[tokio::test]
        async fn should_return_search_filters(#[case] count: usize) {
            let mut repository = MockSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_search_filter_records()
                .return_once(move |_, _| {
                    Box::pin(async move { Ok(fake::vec![SearchFilterRecord; count]) })
                });
            let service = SearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_search_filters(&UserId::new(), &Some(SortOrder::Asc))
                .await;
            assert!(actual.is_ok());
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
            aws_sdk_dynamodb::operation::query::QueryError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::query::QueryError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_search_filter_records()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = SearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_search_filters(&UserId::new(), &Some(SortOrder::Desc))
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                SearchFilterError::SdkQueryError(_) => {}
                _ => panic!("expected SearchFilterError::SdkQueryError"),
            }
        }
    }

    mod find_search_filter {
        use crate::service::{SearchFilterError, SearchFilterService, SearchFilterServiceImpl};
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::user_id::UserId;
        use fake::{Fake, Faker};
        use search_filter_core::search_filter_id::SearchFilterId;
        use search_filter_dynamodb::repository::MockSearchFilterDynamoDbRepository;

        #[tokio::test]
        async fn should_return_search_filter_when_exists() {
            let mut repository = MockSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = SearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_search_filter(&UserId::new(), &SearchFilterId::new())
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_search_filter_not_found_error_when_not_exists() {
            let mut repository = MockSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = SearchFilterServiceImpl {
                repository: &repository,
            };
            let user_id = UserId::new();
            let search_filter_id = SearchFilterId::new();
            let actual = service
                .find_search_filter(&user_id, &search_filter_id)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                SearchFilterError::SearchFilterNotFound(err_user_id, err_search_filter_id) => {
                    assert_eq!(err_user_id, user_id);
                    assert_eq!(err_search_filter_id, search_filter_id);
                }
                _ => panic!("expected SearchFilterError::SearchFilterNotFound"),
            }
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
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_search_filter_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = SearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_search_filter(&UserId::new(), &SearchFilterId::new())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                SearchFilterError::SdkGetItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkGetItemError"),
            }
        }
    }

    mod save_search_filter {
        use crate::service::{SearchFilterError, SearchFilterService, SearchFilterServiceImpl};
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::put_item::PutItemOutput,
        };
        use common::user_id::UserId;
        use fake::{Fake, Faker};
        use search_filter_dynamodb::repository::MockSearchFilterDynamoDbRepository;

        #[tokio::test]
        async fn should_save_search_filter() {
            let mut repository = MockSearchFilterDynamoDbRepository::default();
            repository
                .expect_put_search_filter_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));
            let service = SearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .save_search_filter(&UserId::new(), Faker.fake())
                .await;
            assert!(actual.is_ok());
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
            aws_sdk_dynamodb::operation::put_item::PutItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::put_item::PutItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockSearchFilterDynamoDbRepository::default();
            repository
                .expect_put_search_filter_record()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let service = SearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .save_search_filter(&UserId::new(), Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                SearchFilterError::SdkPutItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkPutItemError"),
            }
        }
    }

    mod delete_search_filter {
        use crate::service::{SearchFilterError, SearchFilterService, SearchFilterServiceImpl};
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::delete_item::DeleteItemOutput,
        };
        use common::user_id::UserId;
        use fake::{Fake, Faker};
        use search_filter_core::search_filter_id::SearchFilterId;
        use search_filter_dynamodb::repository::MockSearchFilterDynamoDbRepository;

        #[tokio::test]
        async fn should_delete_search_filter_when_exists() {
            let mut repository = MockSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_delete_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(DeleteItemOutput::builder().build()) }));
            let service = SearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .delete_search_filter(&UserId::new(), &SearchFilterId::new())
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_search_filter_not_found_error_when_not_exists() {
            let mut repository = MockSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = SearchFilterServiceImpl {
                repository: &repository,
            };

            let user_id = UserId::new();
            let search_filter_id = SearchFilterId::new();
            let actual = service
                .delete_search_filter(&user_id, &search_filter_id)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                SearchFilterError::SearchFilterNotFound(err_user_id, err_search_filter_id) => {
                    assert_eq!(err_user_id, user_id);
                    assert_eq!(err_search_filter_id, search_filter_id);
                }
                _ => panic!("expected SearchFilterError::SearchFilterNotFound"),
            }
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
        async fn should_propagate_sdk_error_for_find(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_search_filter_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = SearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .delete_search_filter(&UserId::new(), &SearchFilterId::new())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                SearchFilterError::SdkGetItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkGetItemError"),
            }
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
            aws_sdk_dynamodb::operation::delete_item::DeleteItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        async fn should_propagate_sdk_error_for_delete(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::delete_item::DeleteItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_delete_search_filter_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = SearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .delete_search_filter(&UserId::new(), &SearchFilterId::new())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                SearchFilterError::SdkDeleteItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkDeleteItemError"),
            }
        }
    }
}
