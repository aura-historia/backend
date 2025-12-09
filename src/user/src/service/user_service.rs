use crate::core::user::User;
use crate::dynamodb::repository::UserDynamoDbRepository;
use crate::service::command::CreateUserCommand;
use aws_sdk_dynamodb::{config::http::HttpResponse, error::SdkError};
use common::user_id::UserId;
use time::OffsetDateTime;

#[derive(thiserror::Error, Debug)]
pub enum UserServiceError {
    #[error("User with UserId '{0}' not found.")]
    UserNotFound(UserId),

    #[error("User with UserId '{0}' cannot be created because user exists already.")]
    UserExistsAlready(UserId),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for PuttItem: {0}")]
    SdkPutItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError, HttpResponse>,
    ),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait UserService {
    async fn find_user(&self, user_id: &UserId) -> Result<User, UserServiceError>;

    async fn create_user(&self, cmd: CreateUserCommand) -> Result<User, UserServiceError>;
}

pub struct UserServiceImpl<'a> {
    repository: &'a (dyn UserDynamoDbRepository + Sync),
}

impl<'a> UserServiceImpl<'a> {
    pub fn new(repository: &'a (dyn UserDynamoDbRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl<'a> UserService for UserServiceImpl<'a> {
    async fn find_user(&self, user_id: &UserId) -> Result<User, UserServiceError> {
        let user_record = self
            .repository
            .get_user_record(user_id)
            .await?
            .ok_or(UserServiceError::UserNotFound(*user_id))?;

        Ok(user_record.into())
    }

    async fn create_user(&self, cmd: CreateUserCommand) -> Result<User, UserServiceError> {
        let exists_guard = self.repository.get_user_record(&cmd.id).await?;
        match exists_guard {
            Some(_) => Err(UserServiceError::UserExistsAlready(cmd.id)),
            None => {
                let now = OffsetDateTime::now_utc();
                let user = User {
                    id: cmd.id,
                    email: cmd.email,
                    first_name: None,
                    last_name: None,
                    language: None,
                    currency: None,
                    created: now,
                    updated: now,
                };
                let _ = self.repository.put_user_record(user.clone().into()).await?;
                Ok(user)
            }
        }
    }
}

#[cfg(test)]
mod tests {

    mod find_user {
        use crate::{
            dynamodb::repository::MockUserDynamoDbRepository,
            service::user_service::{UserService, UserServiceError, UserServiceImpl},
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::user_id::UserId;

        #[tokio::test]
        async fn should_err_user_not_found_when_not_exists() {
            let user_id = UserId::new();
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(None) }));
            let service = UserServiceImpl {
                repository: &repository,
            };
            let actual = service.find_user(&user_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserServiceError::UserNotFound(err_user_id) => {
                    assert_eq!(user_id, err_user_id);
                }
                _ => panic!("expected UserServiceError::UserNotFound"),
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
            let user_id = UserId::new();
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let service = UserServiceImpl {
                repository: &repository,
            };
            let actual = service.find_user(&user_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserServiceError::SdkGetItemError(_) => {}
                _ => panic!("expected UserServiceError::SdkGetItemError"),
            }
        }
    }

    mod create_user {
        use crate::dynamodb::repository::MockUserDynamoDbRepository;
        use crate::service::command::CreateUserCommand;
        use crate::service::user_service::UserServiceError;
        use crate::service::user_service::{UserService, UserServiceImpl};
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_err_user_exists_already_when_exists() {
            let cmd = Faker.fake::<CreateUserCommand>();
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = UserServiceImpl {
                repository: &repository,
            };
            let actual = service.create_user(cmd.clone()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserServiceError::UserExistsAlready(err_user_id) => {
                    assert_eq!(cmd.id, err_user_id);
                }
                _ => panic!("expected UserServiceError::UserExistsAlready"),
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
        async fn should_propagate_sdk_error_for_get(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let service = UserServiceImpl {
                repository: &repository,
            };
            let actual = service.create_user(Faker.fake()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserServiceError::SdkGetItemError(_) => {}
                _ => panic!("expected UserServiceError::SdkGetItemError"),
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
            aws_sdk_dynamodb::operation::put_item::PutItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        async fn should_propagate_sdk_error_for_putt(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::put_item::PutItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(None) }));
            repository
                .expect_put_user_record()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let service = UserServiceImpl {
                repository: &repository,
            };
            let actual = service.create_user(Faker.fake()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserServiceError::SdkPutItemError(_) => {}
                _ => panic!("expected UserServiceError::SdkPutItemError"),
            }
        }
    }
}
