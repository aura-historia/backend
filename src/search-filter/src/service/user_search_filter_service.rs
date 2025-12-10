use crate::core::user_search_filter::UserSearchFilter;
use crate::core::user_search_filter_id::UserSearchFilterId;
use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::dynamodb::repository::UserSearchFilterDynamoDbRepository;
use crate::service::user_search_filter_update::UserSearchFilterUpdate;
use aws_sdk_dynamodb::{config::http::HttpResponse, error::SdkError};
use common::{sort::SortOrder, user_id::UserId};
use product::core::product_search::ProductSearch;
use time::OffsetDateTime;

#[derive(thiserror::Error, Debug)]
pub enum UserSearchFilterError {
    #[error("UserSearchFilter with UserId '{0}' and SearchFilterId '{1}' not found.")]
    UserSearchFilterNotFound(UserId, UserSearchFilterId),

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

    #[error("Encountered DynamoDB SdkError for UpdateItem: {0}")]
    SdkUpdateItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError, HttpResponse>,
    ),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::user_search_filter_service::UserSearchFilterError;
    use common::api::error::ApiError;
    use common::api::error_code::SEARCH_FILTER_NOT_FOUND;

    impl From<UserSearchFilterError> for ApiError {
        fn from(err: UserSearchFilterError) -> Self {
            match err {
                UserSearchFilterError::UserSearchFilterNotFound(_, _) => {
                    ApiError::not_found(SEARCH_FILTER_NOT_FOUND, Box::new(err))
                }
                UserSearchFilterError::SdkGetItemError(err) => err.into(),
                UserSearchFilterError::SdkQueryError(err) => err.into(),
                UserSearchFilterError::SdkPutItemError(err) => err.into(),
                UserSearchFilterError::SdkDeleteItemError(err) => err.into(),
                UserSearchFilterError::SdkUpdateItemError(err) => err.into(),
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait UserSearchFilterService {
    async fn find_user_search_filters(
        &self,
        user_id: &UserId,
        sort_by_created: &Option<SortOrder>,
    ) -> Result<Vec<UserSearchFilter>, UserSearchFilterError>;

    async fn find_user_search_filter(
        &self,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
    ) -> Result<UserSearchFilter, UserSearchFilterError>;

    async fn save_user_search_filter(
        &self,
        user_id: &UserId,
        name: UserSearchFilterName,
        search_filter: ProductSearch,
    ) -> Result<UserSearchFilter, UserSearchFilterError>;

    async fn delete_user_search_filter(
        &self,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
    ) -> Result<(), UserSearchFilterError>;

    async fn update_user_search_filter(
        &self,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
        update: UserSearchFilterUpdate,
    ) -> Result<UserSearchFilter, UserSearchFilterError>;
}

pub struct UserSearchFilterServiceImpl<'a> {
    repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync),
}

impl<'a> UserSearchFilterServiceImpl<'a> {
    pub fn new(repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl<'a> UserSearchFilterService for UserSearchFilterServiceImpl<'a> {
    async fn find_user_search_filters(
        &self,
        user_id: &UserId,
        sort_by_created: &Option<SortOrder>,
    ) -> Result<Vec<UserSearchFilter>, UserSearchFilterError> {
        let search_filters = self
            .repository
            .query_user_search_filter_records(
                user_id,
                matches!(sort_by_created.unwrap_or(SortOrder::Asc), SortOrder::Asc),
            )
            .await?
            .into_iter()
            .map(UserSearchFilter::from)
            .collect();
        Ok(search_filters)
    }

    async fn find_user_search_filter(
        &self,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
    ) -> Result<UserSearchFilter, UserSearchFilterError> {
        let search_filter = self
            .repository
            .get_user_search_filter_record(user_id, user_search_filter_id)
            .await?
            .map(UserSearchFilter::from)
            .ok_or_else(|| {
                UserSearchFilterError::UserSearchFilterNotFound(*user_id, *user_search_filter_id)
            })?;
        Ok(search_filter)
    }

    async fn save_user_search_filter(
        &self,
        user_id: &UserId,
        name: UserSearchFilterName,
        search: ProductSearch,
    ) -> Result<UserSearchFilter, UserSearchFilterError> {
        let user_search_filter = UserSearchFilter {
            user_id: *user_id,
            user_search_filter_id: UserSearchFilterId::new(),
            name,
            search,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };

        let _ = self
            .repository
            .put_user_search_filter_record(user_search_filter.clone().into())
            .await?;

        Ok(user_search_filter)
    }

    async fn delete_user_search_filter(
        &self,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
    ) -> Result<(), UserSearchFilterError> {
        // exists guard
        let _ = self
            .find_user_search_filter(user_id, user_search_filter_id)
            .await?;
        let _ = self
            .repository
            .delete_user_search_filter_record(user_id, user_search_filter_id)
            .await?;
        Ok(())
    }

    async fn update_user_search_filter(
        &self,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
        update: UserSearchFilterUpdate,
    ) -> Result<UserSearchFilter, UserSearchFilterError> {
        // exists guard
        let _ = self
            .find_user_search_filter(user_id, user_search_filter_id)
            .await?;
        let updated_opt = self
            .repository
            .update_user_search_filter_record(user_id, user_search_filter_id, update.into())
            .await?;
        match updated_opt {
            Some(updated) => Ok(updated.into()),
            None => {
                self.find_user_search_filter(user_id, user_search_filter_id)
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest;

    mod find_search_filters {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::sort::SortOrder;
        use common::user_id::UserId;

        #[trace]
        #[rstest::rstest]
        #[case::empty(0)]
        #[case::non_empty(42)]
        #[tokio::test]
        async fn should_return_search_filters(#[case] count: usize) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_records()
                .return_once(move |_, _| {
                    Box::pin(async move { Ok(fake::vec![UserSearchFilterRecord; count]) })
                });
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_user_search_filters(&UserId::new(), &Some(SortOrder::Asc))
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        #[trace]
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
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_records()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_user_search_filters(&UserId::new(), &Some(SortOrder::Desc))
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkQueryError(_) => {}
                _ => panic!("expected SearchFilterError::SdkQueryError"),
            }
        }
    }

    mod find_search_filter {
        use crate::core::user_search_filter_id::UserSearchFilterId;
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::user_id::UserId;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_return_search_filter_when_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_user_search_filter(&UserId::new(), &UserSearchFilterId::new())
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_search_filter_not_found_error_when_not_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let user_id = UserId::new();
            let user_search_filter_id = UserSearchFilterId::new();
            let actual = service
                .find_user_search_filter(&user_id, &user_search_filter_id)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::UserSearchFilterNotFound(
                    err_user_id,
                    err_user_search_filter_id,
                ) => {
                    assert_eq!(err_user_id, user_id);
                    assert_eq!(err_user_search_filter_id, user_search_filter_id);
                }
                _ => panic!("expected SearchFilterError::SearchFilterNotFound"),
            }
        }

        #[tokio::test]
        #[trace]
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
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_user_search_filter(&UserId::new(), &UserSearchFilterId::new())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkGetItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkGetItemError"),
            }
        }
    }

    mod save_search_filter {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::put_item::PutItemOutput,
        };
        use common::user_id::UserId;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_save_search_filter() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_put_user_search_filter_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .save_user_search_filter(&UserId::new(), Faker.fake(), Faker.fake())
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        #[trace]
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
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_put_user_search_filter_record()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .save_user_search_filter(&UserId::new(), Faker.fake(), Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkPutItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkPutItemError"),
            }
        }
    }

    mod delete_search_filter {
        use crate::core::user_search_filter_id::UserSearchFilterId;
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::delete_item::DeleteItemOutput,
        };
        use common::user_id::UserId;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_delete_search_filter_when_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_delete_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(DeleteItemOutput::builder().build()) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .delete_user_search_filter(&UserId::new(), &UserSearchFilterId::new())
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_search_filter_not_found_error_when_not_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };

            let user_id = UserId::new();
            let user_search_filter_id = UserSearchFilterId::new();
            let actual = service
                .delete_user_search_filter(&user_id, &user_search_filter_id)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::UserSearchFilterNotFound(
                    err_user_id,
                    err_user_search_filter_id,
                ) => {
                    assert_eq!(err_user_id, user_id);
                    assert_eq!(err_user_search_filter_id, user_search_filter_id);
                }
                _ => panic!("expected SearchFilterError::SearchFilterNotFound"),
            }
        }

        #[tokio::test]
        #[trace]
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
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .delete_user_search_filter(&UserId::new(), &UserSearchFilterId::new())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkGetItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkGetItemError"),
            }
        }

        #[tokio::test]
        #[trace]
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
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_delete_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .delete_user_search_filter(&UserId::new(), &UserSearchFilterId::new())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkDeleteItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkDeleteItemError"),
            }
        }
    }

    mod update_search_filter {
        use crate::core::user_search_filter_id::UserSearchFilterId;
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::user_id::UserId;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_update_search_filter_when_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_update_user_search_filter_record()
                .return_once(|_, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .update_user_search_filter(&UserId::new(), &UserSearchFilterId::new(), Faker.fake())
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_search_filter_not_found_error_when_not_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };

            let user_id = UserId::new();
            let user_search_filter_id = UserSearchFilterId::new();
            let actual = service
                .update_user_search_filter(&user_id, &user_search_filter_id, Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::UserSearchFilterNotFound(
                    err_user_id,
                    err_user_search_filter_id,
                ) => {
                    assert_eq!(err_user_id, user_id);
                    assert_eq!(err_user_search_filter_id, user_search_filter_id);
                }
                _ => panic!("expected SearchFilterError::SearchFilterNotFound"),
            }
        }

        #[tokio::test]
        #[trace]
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
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .update_user_search_filter(&UserId::new(), &UserSearchFilterId::new(), Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkGetItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkGetItemError"),
            }
        }

        #[tokio::test]
        #[trace]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::update_item::UpdateItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        async fn should_propagate_sdk_error_for_update(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::update_item::UpdateItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_update_user_search_filter_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));
            let service = UserSearchFilterServiceImpl {
                repository: &repository,
            };
            let actual = service
                .update_user_search_filter(&UserId::new(), &UserSearchFilterId::new(), Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkUpdateItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkUpdateItemError"),
            }
        }
    }
}
