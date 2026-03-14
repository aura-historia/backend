use crate::core::user_search_filter::{UserSearchFilter, UserSearchFilterSummary};
use crate::core::user_search_filter_id::UserSearchFilterId;
use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::dynamodb::repository::UserSearchFilterDynamoDbRepository;
use crate::service::user_search_filter_update::UserSearchFilterUpdate;
use aws_sdk_dynamodb::{config::http::HttpResponse, error::SdkError};
use common::{sort::SortOrder, user_id::UserId};
use product::core::product_search::ProductSearch;
use product::opensearch::product_document::ProductDocument;
use time::OffsetDateTime;
use tracing::info;

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

    #[cfg(feature = "opensearch")]
    #[error("Encountered OpenSearch error: {0}")]
    OpenSearchError(#[from] opensearch::Error),
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
                #[cfg(feature = "opensearch")]
                UserSearchFilterError::OpenSearchError(err) => ApiError::internal_server_error(
                    common::api::error_code::INTERNAL_SERVER_ERROR,
                    Box::new(err),
                ),
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

    async fn match_user_search_filters(
        &self,
        product_document: &product::opensearch::product_document::ProductDocument,
    ) -> Result<Vec<UserSearchFilterSummary>, UserSearchFilterError>;
}

pub struct UserSearchFilterServiceImpl<'a> {
    repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync),
    #[cfg(feature = "opensearch")]
    opensearch_repository: Option<
        &'a (dyn crate::opensearch::repository::UserSearchFilterOpenSearchRepository + Sync),
    >,
}

impl<'a> UserSearchFilterServiceImpl<'a> {
    pub fn new(repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync)) -> Self {
        Self {
            repository,
            #[cfg(feature = "opensearch")]
            opensearch_repository: None,
        }
    }

    #[cfg(feature = "opensearch")]
    pub fn with_opensearch(
        repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync),
        opensearch_repository: &'a (
                dyn crate::opensearch::repository::UserSearchFilterOpenSearchRepository + Sync
            ),
    ) -> Self {
        Self {
            repository,
            opensearch_repository: Some(opensearch_repository),
        }
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

        info!(userId = %user_id, userSearchFilterId = %user_search_filter.user_search_filter_id, "Saved UserSearchFilter.");

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
        info!(userId = %user_id, userSearchFilterId = %user_search_filter_id, "Deleted UserSearchFilter.");
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
        info!(userId = %user_id, userSearchFilterId = %user_search_filter_id, "Updated UserSearchFilter.");
        match updated_opt {
            Some(updated) => Ok(updated.into()),
            None => {
                self.find_user_search_filter(user_id, user_search_filter_id)
                    .await
            }
        }
    }

    async fn match_user_search_filters(
        &self,
        product_document: &ProductDocument,
    ) -> Result<Vec<UserSearchFilterSummary>, UserSearchFilterError> {
        #[cfg(feature = "opensearch")]
        {
            use serde::ser::Error as _;

            let opensearch_repo = self.opensearch_repository.ok_or_else(|| {
                UserSearchFilterError::OpenSearchError(opensearch::Error::from(
                    serde_json::Error::custom(
                        "OpenSearch repository not configured. Use UserSearchFilterServiceImpl::with_opensearch() to construct the service.",
                    ),
                ))
            })?;
            let matched_documents = opensearch_repo
                .percolate(product_document)
                .await?
                .into_iter()
                .map(UserSearchFilterSummary::from)
                .collect();
            Ok(matched_documents)
        }
        #[cfg(not(feature = "opensearch"))]
        {
            let _ = product_document;
            unimplemented!("match_user_search_filters requires the 'opensearch' feature")
        }
    }
}

#[cfg(test)]
mod tests {
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

        #[rstest::rstest]
        #[case::empty(0)]
        #[case::non_empty(42)]
        #[tokio::test]
        #[trace]
        async fn should_return_search_filters(#[case] count: usize) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_records()
                .return_once(move |_, _| {
                    Box::pin(async move { Ok(fake::vec![UserSearchFilterRecord; count]) })
                });
            let service = UserSearchFilterServiceImpl::new(&repository);
            let actual = service
                .find_user_search_filters(&UserId::new(), &Some(SortOrder::Asc))
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
        #[trace]
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
            let service = UserSearchFilterServiceImpl::new(&repository);
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
            let service = UserSearchFilterServiceImpl::new(&repository);
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
            let service = UserSearchFilterServiceImpl::new(&repository);
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
        #[trace]
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
            let service = UserSearchFilterServiceImpl::new(&repository);
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
            let service = UserSearchFilterServiceImpl::new(&repository);
            let actual = service
                .save_user_search_filter(&UserId::new(), Faker.fake(), Faker.fake())
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
        #[trace]
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
            let service = UserSearchFilterServiceImpl::new(&repository);
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
            let service = UserSearchFilterServiceImpl::new(&repository);
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
            let service = UserSearchFilterServiceImpl::new(&repository);

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
        #[trace]
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
            let service = UserSearchFilterServiceImpl::new(&repository);
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
        #[trace]
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
            let service = UserSearchFilterServiceImpl::new(&repository);
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
            let service = UserSearchFilterServiceImpl::new(&repository);
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
            let service = UserSearchFilterServiceImpl::new(&repository);

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
        #[trace]
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
            let service = UserSearchFilterServiceImpl::new(&repository);
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
        #[trace]
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
            let service = UserSearchFilterServiceImpl::new(&repository);
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

    mod match_user_search_filters {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::opensearch::repository::MockUserSearchFilterOpenSearchRepository;
        use crate::opensearch::user_search_filter_document::UserSearchFilterDocument;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use fake::{Fake, Faker};
        use product::opensearch::product_document::ProductDocument;

        #[tokio::test]
        async fn should_return_matching_filters_when_product_matches_multiple_filters() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_count = 3;
            opensearch_repo.expect_percolate().return_once(move |_| {
                Box::pin(async move { Ok(fake::vec![UserSearchFilterDocument; expected_count]) })
            });

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), expected_count);
        }

        #[tokio::test]
        async fn should_return_single_matching_filter_when_product_matches_one_filter() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_document: UserSearchFilterDocument = Faker.fake();
            let expected_summary_id = expected_document.user_search_filter_id;

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![expected_document]) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), 1);
            assert_eq!(
                matched_filters[0].user_search_filter_id,
                expected_summary_id
            );
        }

        #[tokio::test]
        async fn should_return_empty_list_when_product_matches_no_filters() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert!(matched_filters.is_empty());
        }

        #[tokio::test]
        async fn should_return_error_when_opensearch_repository_not_configured() {
            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let service = UserSearchFilterServiceImpl::new(&dynamodb_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::OpenSearchError(_) => {}
                other => panic!(
                    "expected UserSearchFilterError::OpenSearchError, but got: {:?}",
                    other
                ),
            }
        }

        #[tokio::test]
        async fn should_propagate_opensearch_error_when_percolate_fails() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            use serde::ser::Error as _;
            let expected_error =
                opensearch::Error::from(serde_json::Error::custom("Percolate query failed"));
            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async { Err(expected_error) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::OpenSearchError(_) => {}
                other => panic!(
                    "expected UserSearchFilterError::OpenSearchError, but got: {:?}",
                    other
                ),
            }
        }

        #[rstest::rstest]
        #[case::empty(0)]
        #[case::single(1)]
        #[case::multiple(10)]
        #[case::large_batch(100)]
        #[tokio::test]
        #[trace]
        async fn should_convert_opensearch_documents_to_summaries(#[case] count: usize) {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let documents: Vec<UserSearchFilterDocument> =
                fake::vec![UserSearchFilterDocument; count];
            let expected_ids: Vec<_> = documents.iter().map(|d| d.user_search_filter_id).collect();

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), count);
            for (i, filter) in matched_filters.iter().enumerate() {
                assert_eq!(filter.user_search_filter_id, expected_ids[i]);
            }
        }

        #[tokio::test]
        async fn should_preserve_user_id_when_converting_matching_documents() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_document: UserSearchFilterDocument = Faker.fake();
            let expected_user_id = expected_document.user_id;

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![expected_document]) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters[0].user_id, expected_user_id);
        }

        #[tokio::test]
        async fn should_preserve_filter_name_when_converting_matching_documents() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_document: UserSearchFilterDocument = Faker.fake();
            let expected_name = expected_document.name.clone();

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![expected_document]) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters[0].name, expected_name);
        }

        #[tokio::test]
        async fn should_preserve_timestamps_when_converting_matching_documents() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_document: UserSearchFilterDocument = Faker.fake();
            let expected_created = expected_document.created;
            let expected_updated = expected_document.updated;

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![expected_document]) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters[0].created, expected_created);
            assert_eq!(matched_filters[0].updated, expected_updated);
        }

        #[tokio::test]
        async fn should_accept_product_document_reference_without_consuming_it() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let matched_document: UserSearchFilterDocument = Faker.fake();
            let document_clone = matched_document.clone();

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![matched_document]) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            // Call the method with product_document reference
            let result = service.match_user_search_filters(&product_document).await;

            // Verify the result is OK and product_document reference wasn't consumed
            assert!(result.is_ok());
            let matched_filters = result.unwrap();
            assert_eq!(matched_filters.len(), 1);
            assert_eq!(
                matched_filters[0].user_search_filter_id,
                document_clone.user_search_filter_id
            );
        }

        #[tokio::test]
        async fn should_handle_large_batch_of_matching_filters() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let documents: Vec<UserSearchFilterDocument> =
                fake::vec![UserSearchFilterDocument; 500];
            let expected_count = documents.len();

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), expected_count);
        }

        #[tokio::test]
        async fn should_maintain_filter_order_from_opensearch_results() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let documents: Vec<UserSearchFilterDocument> = fake::vec![UserSearchFilterDocument; 5];
            let expected_ids: Vec<_> = documents.iter().map(|d| d.user_search_filter_id).collect();

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            for (i, summary) in matched_filters.iter().enumerate() {
                assert_eq!(summary.user_search_filter_id, expected_ids[i]);
            }
        }

        #[tokio::test]
        async fn should_correctly_handle_single_filter_with_all_fields() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_document: UserSearchFilterDocument = Faker.fake();
            let expected_id = expected_document.user_search_filter_id;
            let expected_user_id = expected_document.user_id;
            let expected_name = expected_document.name.clone();
            let expected_created = expected_document.created;
            let expected_updated = expected_document.updated;

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![expected_document]) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), 1);

            let summary = &matched_filters[0];
            assert_eq!(summary.user_search_filter_id, expected_id);
            assert_eq!(summary.user_id, expected_user_id);
            assert_eq!(summary.name, expected_name);
            assert_eq!(summary.created, expected_created);
            assert_eq!(summary.updated, expected_updated);
        }

        #[tokio::test]
        async fn should_return_summaries_with_all_required_fields() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let documents = vec![Faker.fake::<UserSearchFilterDocument>()];

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), 1);

            let summary = &matched_filters[0];
            assert!(!summary.user_search_filter_id.to_string().is_empty());
            assert!(!summary.user_id.to_string().is_empty());
            assert!(!summary.name.to_string().is_empty());
        }

        #[tokio::test]
        async fn should_not_modify_product_document_when_matching() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let product_document_copy = Faker.fake::<ProductDocument>();

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document = product_document_copy.clone();

            let _ = service.match_user_search_filters(&product_document).await;

            assert_eq!(
                product_document.product_id,
                product_document_copy.product_id
            );
            assert_eq!(product_document.shop_id, product_document_copy.shop_id);
        }

        #[tokio::test]
        async fn should_correctly_convert_all_fields_in_result() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let mut documents = vec![];
            for _ in 0..3 {
                documents.push(Faker.fake::<UserSearchFilterDocument>());
            }

            let original_documents = documents.clone();
            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();

            for (i, summary) in matched_filters.iter().enumerate() {
                assert_eq!(
                    summary.user_search_filter_id,
                    original_documents[i].user_search_filter_id
                );
                assert_eq!(summary.user_id, original_documents[i].user_id);
                assert_eq!(summary.name, original_documents[i].name);
                assert_eq!(summary.created, original_documents[i].created);
                assert_eq!(summary.updated, original_documents[i].updated);
            }
        }

        #[tokio::test]
        async fn should_return_result_type_on_success() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            assert!(actual.unwrap().is_empty());
        }

        #[tokio::test]
        async fn should_return_error_type_on_missing_repository() {
            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let service = UserSearchFilterServiceImpl::new(&dynamodb_repo);
            let product_document: ProductDocument = Faker.fake();

            let result = service.match_user_search_filters(&product_document).await;

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                UserSearchFilterError::OpenSearchError(_)
            ));
        }

        #[tokio::test]
        async fn should_handle_multiple_documents_with_different_user_ids() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let documents: Vec<UserSearchFilterDocument> = fake::vec![UserSearchFilterDocument; 4];
            let user_ids: Vec<_> = documents.iter().map(|d| d.user_id).collect();

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            for (i, summary) in matched_filters.iter().enumerate() {
                assert_eq!(summary.user_id, user_ids[i]);
            }
        }

        #[tokio::test]
        async fn should_properly_map_opensearch_document_to_summary_type() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let doc: UserSearchFilterDocument = Faker.fake();
            let doc_id = doc.user_search_filter_id;
            let doc_user_id = doc.user_id;
            let doc_name = doc.name.clone();

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![doc]) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let summaries = actual.unwrap();
            assert_eq!(summaries.len(), 1);

            // Verify that the type conversion happened correctly
            let summary = &summaries[0];
            assert_eq!(summary.user_search_filter_id, doc_id);
            assert_eq!(summary.user_id, doc_user_id);
            assert_eq!(summary.name, doc_name);
        }

        #[tokio::test]
        async fn should_handle_percolate_returning_exact_count() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let count = 7;
            let documents: Vec<UserSearchFilterDocument> =
                fake::vec![UserSearchFilterDocument; count];

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let service =
                UserSearchFilterServiceImpl::with_opensearch(&dynamodb_repo, &opensearch_repo);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), count);
        }
    }
}
