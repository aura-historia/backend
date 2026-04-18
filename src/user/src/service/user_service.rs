use crate::core::role::UserRole;
use crate::core::tier::UserTier;
use crate::core::user::User;
use crate::dynamodb::repository::UserDynamoDbRepository;
use crate::dynamodb::role_record::UserRoleRecord;
use crate::dynamodb::tier_record::UserTierRecord;
use crate::dynamodb::user_record::{mk_gsi1_pk, mk_gsi1_sk};
use crate::dynamodb::user_record_update::UserRecordUpdate;
use crate::service::cognito_admin_service::{CognitoAdminError, CognitoAdminService};
use crate::service::command::{CreateUserCommand, UpdateUserCommand};
use aws_sdk_dynamodb::error::SdkError;
use common::currency::record::CurrencyRecord;
use common::language::record::LanguageRecord;
use common::stripe_customer_id::StripeCustomerId;
use common::user_id::UserId;
use time::OffsetDateTime;
use tracing::{error, info, warn};

const MAX_DELETE_RETRIES: u32 = 5;

#[derive(thiserror::Error, Debug)]
pub enum UserServiceError {
    #[error("User with UserId '{0}' not found.")]
    UserNotFound(UserId),

    #[error("User with UserId '{0}' cannot be created because user exists already.")]
    UserExistsAlready(UserId),

    #[error("User for given Stripe customer id not found.")]
    UserNotFoundByStripeCustomerId,

    #[error("This action requires the 'ADMIN' role.")]
    AdminRoleRequired,

    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkGetItemError(#[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>),

    #[error("Encountered DynamoDB SdkError for Query: {0:?}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError>),

    #[error("Encountered DynamoDB SdkError for PutItem: {0:?}")]
    SdkPutItemError(#[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError>),

    #[error("Encountered DynamoDB SdkError for UpdateItem: {0:?}")]
    SdkUpdateItemError(#[from] SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError>),

    #[error("Encountered DynamoDB SdkError for DeleteItem: {0:?}")]
    SdkDeleteItemError(#[from] SdkError<aws_sdk_dynamodb::operation::delete_item::DeleteItemError>),

    #[error("Failed to delete Cognito user: {0}")]
    CognitoAdminError(#[from] CognitoAdminError),

    #[error("Cognito admin service not configured")]
    CognitoAdminServiceNotConfigured,
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::user_service::UserServiceError;
    use common::api::error::ApiError;
    use common::api::error_code::{
        FORBIDDEN, INTERNAL_SERVER_ERROR, USER_EXISTS_ALREADY, USER_NOT_FOUND,
    };

    impl From<UserServiceError> for ApiError {
        fn from(err: UserServiceError) -> Self {
            match err {
                UserServiceError::UserNotFound(_) => {
                    ApiError::not_found(USER_NOT_FOUND, Box::new(err))
                }
                UserServiceError::UserNotFoundByStripeCustomerId => {
                    ApiError::not_found(USER_NOT_FOUND, Box::new(err))
                }
                UserServiceError::UserExistsAlready(_) => {
                    ApiError::conflict(USER_EXISTS_ALREADY, Box::new(err))
                }
                UserServiceError::AdminRoleRequired => {
                    ApiError::forbidden(FORBIDDEN).with_detail(err.to_string())
                }
                UserServiceError::SdkGetItemError(sdk_error) => sdk_error.into(),
                UserServiceError::SdkQueryError(sdk_error) => sdk_error.into(),
                UserServiceError::SdkPutItemError(sdk_error) => sdk_error.into(),
                UserServiceError::SdkUpdateItemError(sdk_error) => sdk_error.into(),
                UserServiceError::SdkDeleteItemError(sdk_error) => sdk_error.into(),
                UserServiceError::CognitoAdminError(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
                UserServiceError::CognitoAdminServiceNotConfigured => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait UserService {
    async fn find_user(&self, user_id: &UserId) -> Result<User, UserServiceError>;

    async fn find_user_by_stripe_customer_id(
        &self,
        stripe_customer_id: &StripeCustomerId,
    ) -> Result<User, UserServiceError>;

    async fn check_admin(&self, user_id: &UserId) -> Result<(), UserServiceError>;

    async fn create_user(&self, cmd: CreateUserCommand) -> Result<User, UserServiceError>;

    async fn update_user(
        &self,
        user_id: &UserId,
        cmd: UpdateUserCommand,
    ) -> Result<User, UserServiceError>;

    async fn delete_user(&self, user_id: &UserId) -> Result<(), UserServiceError>;
}

pub struct UserServiceImpl<'a> {
    repository: &'a (dyn UserDynamoDbRepository + Sync),
    cognito_admin_service: Option<&'a (dyn CognitoAdminService + Sync)>,
}

impl<'a> UserServiceImpl<'a> {
    pub fn new(repository: &'a (dyn UserDynamoDbRepository + Sync)) -> Self {
        Self {
            repository,
            cognito_admin_service: None,
        }
    }

    pub fn with_cognito(
        repository: &'a (dyn UserDynamoDbRepository + Sync),
        cognito_admin_service: &'a (dyn CognitoAdminService + Sync),
    ) -> Self {
        Self {
            repository,
            cognito_admin_service: Some(cognito_admin_service),
        }
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

    async fn find_user_by_stripe_customer_id(
        &self,
        stripe_customer_id: &StripeCustomerId,
    ) -> Result<User, UserServiceError> {
        let user_record = self
            .repository
            .find_user_record_by_stripe_customer_id(stripe_customer_id)
            .await?
            .ok_or(UserServiceError::UserNotFoundByStripeCustomerId)?;

        Ok(user_record.into())
    }

    async fn check_admin(&self, user_id: &UserId) -> Result<(), UserServiceError> {
        let user = self.find_user(user_id).await?;
        if user.role != UserRole::Admin {
            return Err(UserServiceError::AdminRoleRequired);
        }
        Ok(())
    }

    async fn create_user(&self, cmd: CreateUserCommand) -> Result<User, UserServiceError> {
        let exists_guard = self.repository.get_user_record(&cmd.id).await?;
        match exists_guard {
            Some(_) => Err(UserServiceError::UserExistsAlready(cmd.id)),
            None => {
                let now = OffsetDateTime::now_utc();
                let user = User {
                    user_id: cmd.id,
                    email: cmd.email,
                    first_name: None,
                    last_name: None,
                    language: None,
                    currency: None,
                    prohibited_content_consent: false,
                    tier: UserTier::Free,
                    role: UserRole::User,
                    stripe_customer_id: None,
                    created: now,
                    updated: now,
                };
                let _ = self.repository.put_user_record(user.clone().into()).await?;
                info!(userId = %user.user_id, "Created User.");
                Ok(user)
            }
        }
    }

    async fn update_user(
        &self,
        user_id: &UserId,
        cmd: UpdateUserCommand,
    ) -> Result<User, UserServiceError> {
        // exists-guard
        let existing_user = self.find_user(user_id).await?;

        if cmd.is_empty() {
            Ok(existing_user)
        } else {
            let (gsi1_pk, gsi1_sk) = match cmd.stripe_customer_id.as_ref() {
                Some(scid) => (Some(mk_gsi1_pk(scid)), Some(mk_gsi1_sk().to_owned())),
                None => (None, None),
            };
            let user_record_update = UserRecordUpdate {
                first_name: cmd.first_name,
                last_name: cmd.last_name,
                language: cmd.language.map(LanguageRecord::from),
                currency: cmd.currency.map(CurrencyRecord::from),
                prohibited_content_consent: cmd.prohibited_content_consent,
                tier: cmd.tier.map(UserTierRecord::from),
                role: cmd.role.map(UserRoleRecord::from),
                stripe_customer_id: cmd.stripe_customer_id,
                gsi1_pk,
                gsi1_sk,
                updated: OffsetDateTime::now_utc(),
            };
            let user = self.repository
                .update_user_record(user_id, user_record_update)
                .await?
                .ok_or(UserServiceError::SdkUpdateItemError(
                    SdkError::construction_failure("Failed deserializing updated UserRecord in UpdateItem-Response from DynamoDB."),
                )).map(User::from)?;

            info!(userId = %user.user_id, "Updated User.");

            Ok(user)
        }
    }

    async fn delete_user(&self, user_id: &UserId) -> Result<(), UserServiceError> {
        let cognito = self
            .cognito_admin_service
            .ok_or(UserServiceError::CognitoAdminServiceNotConfigured)?;

        cognito.admin_delete_user(user_id).await?;
        info!(userId = %user_id, "Deleted Cognito user.");

        let mut last_err = None;
        for attempt in 1..=MAX_DELETE_RETRIES {
            match self.repository.delete_user_record(user_id).await {
                Ok(_) => {
                    info!(userId = %user_id, "Deleted User.");
                    return Ok(());
                }
                Err(err) => {
                    if attempt < MAX_DELETE_RETRIES {
                        warn!(
                            userId = %user_id,
                            attempt,
                            error = %err,
                            "Failed deleting user record from DynamoDB, retrying."
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(
                            100 * 2_u64.saturating_pow(attempt - 1),
                        ))
                        .await;
                    } else {
                        error!(
                            userId = %user_id,
                            attempt,
                            error = %err,
                            "Failed deleting user record from DynamoDB after max retries."
                        );
                    }
                    last_err = Some(err);
                }
            }
        }

        Err(last_err.unwrap().into())
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
            let service = UserServiceImpl::new(&repository);
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
            let service = UserServiceImpl::new(&repository);
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
            let service = UserServiceImpl::new(&repository);
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
            let service = UserServiceImpl::new(&repository);
            let actual = service.create_user(Faker.fake()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserServiceError::SdkGetItemError(_) => {}
                _ => panic!("expected UserServiceError::SdkGetItemError"),
            }
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
            let service = UserServiceImpl::new(&repository);
            let actual = service.create_user(Faker.fake()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserServiceError::SdkPutItemError(_) => {}
                _ => panic!("expected UserServiceError::SdkPutItemError"),
            }
        }

        #[tokio::test]
        async fn should_default_prohibited_content_consent_to_false_for_new_user() {
            use aws_sdk_dynamodb::operation::put_item::PutItemOutput;
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(None) }));
            repository
                .expect_put_user_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));
            let service = UserServiceImpl::new(&repository);
            let actual = service.create_user(Faker.fake()).await.unwrap();

            assert!(!actual.prohibited_content_consent);
        }
    }

    mod update_user {
        use crate::{
            dynamodb::repository::MockUserDynamoDbRepository,
            service::{
                command::UpdateUserCommand,
                user_service::{UserService, UserServiceError, UserServiceImpl},
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::user_id::UserId;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_err_user_not_found_when_not_exists() {
            let user_id = UserId::new();
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(None) }));
            let service = UserServiceImpl::new(&repository);
            let actual = service.update_user(&user_id, Faker.fake()).await;

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
        async fn should_propagate_sdk_error_for_find(
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
            let service = UserServiceImpl::new(&repository);
            let actual = service.update_user(&user_id, Faker.fake()).await;

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
            aws_sdk_dynamodb::operation::update_item::UpdateItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        async fn should_propagate_sdk_error_for_update(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::update_item::UpdateItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let user_id = UserId::new();
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_update_user_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = UserServiceImpl::new(&repository);
            let update = UpdateUserCommand {
                first_name: Some("foo".into()),
                last_name: None,
                language: None,
                currency: None,
                prohibited_content_consent: None,
                tier: None,
                role: None,
                stripe_customer_id: None,
            };
            let actual = service.update_user(&user_id, update).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserServiceError::SdkUpdateItemError(_) => {}
                other => {
                    panic!("expected UserServiceError::SdkUpdateItemError, got other: {other}")
                }
            }
        }

        #[tokio::test]
        async fn should_return_existing_user_when_command_is_empty() {
            let user_id = UserId::new();
            let mut existing_record = Faker.fake::<crate::dynamodb::user_record::UserRecord>();
            existing_record.user_id = user_id;
            let existing_user = crate::core::user::User::from(existing_record.clone());
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(move |_| Box::pin(async move { Ok(Some(existing_record)) }));
            let service = UserServiceImpl::new(&repository);
            let actual = service
                .update_user(&user_id, UpdateUserCommand::default())
                .await
                .unwrap();

            assert_eq!(existing_user, actual);
        }

        #[tokio::test]
        async fn should_pass_prohibited_content_consent_to_update() {
            let user_id = UserId::new();
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_update_user_record()
                .return_once(|_, update| {
                    assert_eq!(Some(true), update.prohibited_content_consent);
                    Box::pin(async { Ok(Some(Faker.fake())) })
                });
            let service = UserServiceImpl::new(&repository);
            let update = UpdateUserCommand {
                prohibited_content_consent: Some(true),
                ..Default::default()
            };
            let _ = service.update_user(&user_id, update).await.unwrap();
        }
    }

    mod delete_user {
        use crate::{
            dynamodb::repository::MockUserDynamoDbRepository,
            service::{
                cognito_admin_service::{CognitoAdminError, MockCognitoAdminService},
                user_service::{UserService, UserServiceError, UserServiceImpl},
            },
        };
        use aws_sdk_dynamodb::{error::SdkError, operation::delete_item::DeleteItemOutput};
        use common::user_id::UserId;

        #[tokio::test]
        async fn should_err_cognito_admin_service_not_configured_when_none() {
            let repository = MockUserDynamoDbRepository::default();
            let service = UserServiceImpl::new(&repository);
            let actual = service.delete_user(&UserId::new()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserServiceError::CognitoAdminServiceNotConfigured => {}
                other => {
                    panic!(
                        "expected UserServiceError::CognitoAdminServiceNotConfigured, got: {other}"
                    )
                }
            }
        }

        #[tokio::test]
        async fn should_delete_cognito_user_and_dynamodb_record() {
            let user_id = UserId::new();
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_delete_user_record()
                .return_once(|_| Box::pin(async { Ok(DeleteItemOutput::builder().build()) }));
            let mut cognito = MockCognitoAdminService::default();
            cognito
                .expect_admin_delete_user()
                .return_once(|_| Box::pin(async { Ok(()) }));
            let service = UserServiceImpl::with_cognito(&repository, &cognito);

            let actual = service.delete_user(&user_id).await;

            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_propagate_cognito_admin_error() {
            let user_id = UserId::new();
            let repository = MockUserDynamoDbRepository::default();
            let mut cognito = MockCognitoAdminService::default();
            cognito.expect_admin_delete_user().return_once(|_| {
                Box::pin(async {
                    Err(CognitoAdminError::AdminDeleteUser(
                        "Something went wrong".into(),
                    ))
                })
            });
            let service = UserServiceImpl::with_cognito(&repository, &cognito);

            let actual = service.delete_user(&user_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserServiceError::CognitoAdminError(_) => {}
                other => panic!("expected UserServiceError::CognitoAdminError, got: {other}"),
            }
        }

        #[tokio::test]
        async fn should_propagate_sdk_error_for_delete_after_retries() {
            let user_id = UserId::new();
            let mut repository = MockUserDynamoDbRepository::default();
            repository.expect_delete_user_record().returning(move |_| {
                Box::pin(async { Err(SdkError::construction_failure("Something went wrong")) })
            });
            let mut cognito = MockCognitoAdminService::default();
            cognito
                .expect_admin_delete_user()
                .return_once(|_| Box::pin(async { Ok(()) }));
            let service = UserServiceImpl::with_cognito(&repository, &cognito);

            let actual = service.delete_user(&user_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserServiceError::SdkDeleteItemError(_) => {}
                other => {
                    panic!("expected UserServiceError::SdkDeleteItemError, got: {other}")
                }
            }
        }

        #[tokio::test]
        async fn should_succeed_on_retry_after_initial_failure() {
            let user_id = UserId::new();
            let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let call_count_clone = call_count.clone();
            let mut repository = MockUserDynamoDbRepository::default();
            repository.expect_delete_user_record().returning(move |_| {
                let count = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if count == 0 {
                    Box::pin(async { Err(SdkError::construction_failure("Transient failure")) })
                } else {
                    Box::pin(async { Ok(DeleteItemOutput::builder().build()) })
                }
            });
            let mut cognito = MockCognitoAdminService::default();
            cognito
                .expect_admin_delete_user()
                .return_once(|_| Box::pin(async { Ok(()) }));
            let service = UserServiceImpl::with_cognito(&repository, &cognito);

            let actual = service.delete_user(&user_id).await;

            assert!(actual.is_ok());
            assert_eq!(
                2,
                call_count.load(std::sync::atomic::Ordering::SeqCst),
                "should have retried once"
            );
        }
    }

    mod stripe_subscriptions {
        use crate::core::tier::UserTier;
        use crate::dynamodb::repository::MockUserDynamoDbRepository;
        use crate::dynamodb::user_record::{UserRecord, mk_gsi1_pk, mk_gsi1_sk};
        use crate::service::command::UpdateUserCommand;
        use crate::service::user_service::{UserService, UserServiceError, UserServiceImpl};
        use common::stripe_customer_id::StripeCustomerId;
        use common::user_id::UserId;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_set_tier_and_gsi1_keys_when_update_includes_stripe_customer_id() {
            let user_id = UserId::new();
            let stripe_customer_id = StripeCustomerId::from("cus_xyz");
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(Some(Faker.fake::<UserRecord>())) }));
            let scid_clone = stripe_customer_id.clone();
            repository
                .expect_update_user_record()
                .withf(move |_uid, update| {
                    update.tier.is_some()
                        && update.stripe_customer_id.as_ref() == Some(&scid_clone)
                        && update.gsi1_pk.as_deref() == Some(mk_gsi1_pk(&scid_clone).as_str())
                        && update.gsi1_sk.as_deref() == Some(mk_gsi1_sk())
                })
                .return_once(move |_, _| {
                    let mut user_record = Faker.fake::<UserRecord>();
                    user_record.user_id = user_id;
                    Box::pin(async move { Ok(Some(user_record)) })
                });
            let service = UserServiceImpl::new(&repository);

            let actual = service
                .update_user(
                    &user_id,
                    UpdateUserCommand {
                        tier: Some(UserTier::Pro),
                        stripe_customer_id: Some(stripe_customer_id),
                        ..Default::default()
                    },
                )
                .await;

            assert!(actual.is_ok(), "expected Ok, got {actual:?}");
        }

        #[tokio::test]
        async fn should_only_update_tier_without_touching_stripe_customer_id_when_not_supplied() {
            let user_id = UserId::new();
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(Some(Faker.fake::<UserRecord>())) }));
            repository
                .expect_update_user_record()
                .withf(|_uid, update| {
                    update.tier.is_some()
                        && update.stripe_customer_id.is_none()
                        && update.gsi1_pk.is_none()
                        && update.gsi1_sk.is_none()
                })
                .return_once(move |_, _| {
                    let mut user_record = Faker.fake::<UserRecord>();
                    user_record.user_id = user_id;
                    Box::pin(async move { Ok(Some(user_record)) })
                });
            let service = UserServiceImpl::new(&repository);

            let actual = service
                .update_user(
                    &user_id,
                    UpdateUserCommand {
                        tier: Some(UserTier::Free),
                        ..Default::default()
                    },
                )
                .await;

            assert!(actual.is_ok(), "expected Ok, got {actual:?}");
        }

        #[tokio::test]
        async fn should_find_user_by_stripe_customer_id_when_user_exists() {
            let stripe_customer_id = StripeCustomerId::from("cus_abc");
            let mut repository = MockUserDynamoDbRepository::default();
            let mut user_record = Faker.fake::<UserRecord>();
            user_record.stripe_customer_id = Some(stripe_customer_id.clone());
            repository
                .expect_find_user_record_by_stripe_customer_id()
                .return_once(move |_| Box::pin(async move { Ok(Some(user_record)) }));
            let service = UserServiceImpl::new(&repository);

            let actual = service
                .find_user_by_stripe_customer_id(&stripe_customer_id)
                .await;

            assert!(actual.is_ok(), "expected Ok, got {actual:?}");
        }

        #[tokio::test]
        async fn should_err_not_found_by_stripe_customer_id_when_user_missing() {
            let mut repository = MockUserDynamoDbRepository::default();
            repository
                .expect_find_user_record_by_stripe_customer_id()
                .return_once(|_| Box::pin(async { Ok(None) }));
            let service = UserServiceImpl::new(&repository);

            let actual = service
                .find_user_by_stripe_customer_id(&StripeCustomerId::from("cus_unknown"))
                .await;

            assert!(matches!(
                actual.unwrap_err(),
                UserServiceError::UserNotFoundByStripeCustomerId
            ));
        }
    }
}
