use crate::ports::{UserAdminReadError, UserAdminReader};
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use user_core::role::UserRole;

#[derive(Debug, thiserror::Error)]
pub(crate) enum RequireAdminActorError {
    #[error("authenticated actor required")]
    AuthenticationRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error(transparent)]
    UserAdminRead(#[from] UserAdminReadError),
}

pub(crate) fn require_admin_actor_credential(
    context: &OperationContext,
    capability: CredentialCapability,
) -> Result<(), RequireAdminActorError> {
    context
        .require()
        .credential_capability(capability)
        .authorize::<RequireAdminActorError>()
}

pub(crate) async fn require_admin_actor<R: UserAdminReader>(
    context: &OperationContext,
    reader: &mut R,
) -> Result<(), RequireAdminActorError> {
    match &context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => {
            let actor = reader
                .find_admin_actor(*user_id)
                .await?
                .ok_or(RequireAdminActorError::Forbidden)?;
            if actor.role == UserRole::Admin {
                Ok(())
            } else {
                Err(RequireAdminActorError::Forbidden)
            }
        }
        Principal::Anonymous => Err(RequireAdminActorError::AuthenticationRequired),
    }
}

impl From<OperationAuthorizationError> for RequireAdminActorError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => Self::AuthenticationRequired,
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ports::{
        UserAdminActorView, UserAdminReadError, UserAdminReader, UserRepository,
        UserRepositoryError, UserStorageVersion, VersionedUser,
    };

    use super::*;
    use common::error::boxed::{BoxError, box_error};
    use common::operation_context::{CorrelationId, RequestId};
    use common::stripe_customer_id::StripeCustomerId;
    use common::user_id::UserId;
    use common::versioned::Versioned;
    use serde_email::Email;
    use std::collections::BTreeSet;
    use user_core::tier::UserTier;
    use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};

    struct FakeUserRepository {
        user: Option<User>,
        error: Option<UserRepositoryError>,
        find_by_id_calls: usize,
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    fn email(value: &str) -> Email {
        match Email::try_from(value) {
            Ok(email) => email,
            Err(error) => panic!("invalid test email: {error}"),
        }
    }

    fn user_with(id: UserId, role: UserRole) -> User {
        match User::create(NewUser {
            id,
            email: email("actor@example.com"),
            profile: UserProfile::default(),
            preferences: UserPreferences::default(),
            account: UserAccount {
                tier: UserTier::Free,
                role,
                stripe_customer_id: None,
            },
        }) {
            Ok(user) => user,
            Err(error) => panic!("invalid test user: {error}"),
        }
    }

    fn boxed() -> BoxError {
        box_error(std::io::Error::other("boom"))
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeUserRepository {
        async fn find_admin_actor(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            self.find_by_id_calls += 1;
            if let Some(error) = self.error.take() {
                match error {
                    UserRepositoryError::TemporarilyUnavailable { source } => {
                        Err(UserAdminReadError::TemporarilyUnavailable { source })
                    }
                    UserRepositoryError::InvalidPersistedState { source } => {
                        Err(UserAdminReadError::InvalidReadModel { source })
                    }
                    UserRepositoryError::Internal { source } => {
                        Err(UserAdminReadError::Internal { source })
                    }
                    _ => Err(UserAdminReadError::Internal {
                        source: "unexpected repository error".into(),
                    }),
                }
            } else {
                Ok(self.user.clone().map(|user| UserAdminActorView {
                    user_id: user.id(),
                    role: user.account().role,
                }))
            }
        }
    }

    #[async_trait::async_trait]
    impl UserRepository for FakeUserRepository {
        async fn find_by_id(
            &mut self,
            _id: UserId,
        ) -> Result<Option<crate::ports::VersionedUser>, UserRepositoryError> {
            self.find_by_id_calls += 1;
            if let Some(error) = self.error.take() {
                Err(error)
            } else {
                Ok(self.user.clone().map(|value| Versioned {
                    value,
                    version: crate::ports::UserStorageVersion::INITIAL,
                }))
            }
        }

        async fn find_by_email(
            &mut self,
            _email: &Email,
        ) -> Result<Option<crate::ports::VersionedUser>, UserRepositoryError> {
            Ok(None)
        }

        async fn find_by_stripe_customer_id(
            &mut self,
            _stripe_customer_id: &StripeCustomerId,
        ) -> Result<Option<crate::ports::VersionedUser>, UserRepositoryError> {
            Ok(None)
        }

        async fn insert(&mut self, user: &User) -> Result<VersionedUser, UserRepositoryError> {
            Ok(Versioned::new(user.clone(), UserStorageVersion::INITIAL))
        }

        async fn insert_if_absent(
            &mut self,
            user: &User,
        ) -> Result<crate::ports::UserInsertOutcome, UserRepositoryError> {
            Ok(crate::ports::UserInsertOutcome::Created(Versioned::new(
                user.clone(),
                UserStorageVersion::INITIAL,
            )))
        }

        async fn update(
            &mut self,
            user: &User,
            _expected_version: UserStorageVersion,
        ) -> Result<VersionedUser, UserRepositoryError> {
            Ok(Versioned::new(user.clone(), UserStorageVersion::INITIAL))
        }

        async fn delete_by_id(&mut self, _id: UserId) -> Result<bool, UserRepositoryError> {
            Ok(true)
        }
    }

    fn repository(user: Option<User>) -> FakeUserRepository {
        FakeUserRepository {
            user,
            error: None,
            find_by_id_calls: 0,
        }
    }

    fn assert_error<T, F>(result: Result<T, RequireAdminActorError>, predicate: F)
    where
        F: FnOnce(&RequireAdminActorError) -> bool,
    {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => assert!(predicate(&error), "unexpected error: {error:?}"),
        }
    }

    #[test]
    fn should_require_configured_credential_capability() {
        let user_id = UserId::new();
        assert!(
            require_admin_actor_credential(
                &context(Principal::DelegatedUser {
                    user_id,
                    capabilities: BTreeSet::from([CredentialCapability::UsersWrite]),
                }),
                CredentialCapability::UsersWrite,
            )
            .is_ok()
        );

        assert_error(
            require_admin_actor_credential(
                &context(Principal::DelegatedUser {
                    user_id,
                    capabilities: BTreeSet::new(),
                }),
                CredentialCapability::UsersWrite,
            ),
            |error| matches!(error, RequireAdminActorError::Forbidden),
        );
    }

    #[test]
    fn should_reject_anonymous_when_requiring_credential_capability() {
        assert_error(
            require_admin_actor_credential(
                &context(Principal::Anonymous),
                CredentialCapability::UsersWrite,
            ),
            |error| matches!(error, RequireAdminActorError::AuthenticationRequired),
        );
    }

    #[tokio::test]
    async fn should_allow_service_and_system_without_user_lookup() {
        let mut service_repo = repository(None);
        assert!(
            require_admin_actor(
                &context(Principal::Service("svc".to_owned())),
                &mut service_repo,
            )
            .await
            .is_ok()
        );
        assert_eq!(0, service_repo.find_by_id_calls);

        let mut system_repo = repository(None);
        assert!(
            require_admin_actor(&context(Principal::System), &mut system_repo)
                .await
                .is_ok()
        );
        assert_eq!(0, system_repo.find_by_id_calls);
    }

    #[tokio::test]
    async fn should_allow_admin_user_and_delegated_admin_user() {
        let user_id = UserId::new();
        let admin = user_with(user_id, UserRole::Admin);
        let mut user_repo = repository(Some(admin.clone()));
        assert!(
            require_admin_actor(&context(Principal::User(user_id)), &mut user_repo)
                .await
                .is_ok()
        );

        let mut delegated_repo = repository(Some(admin));
        assert!(
            require_admin_actor(
                &context(Principal::DelegatedUser {
                    user_id,
                    capabilities: BTreeSet::from([CredentialCapability::UsersWrite]),
                }),
                &mut delegated_repo,
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn should_reject_non_admin_missing_and_anonymous_actor() {
        let user_id = UserId::new();
        let mut non_admin_repo = repository(Some(user_with(user_id, UserRole::User)));
        assert_error(
            require_admin_actor(&context(Principal::User(user_id)), &mut non_admin_repo).await,
            |error| matches!(error, RequireAdminActorError::Forbidden),
        );

        let mut missing_repo = repository(None);
        assert_error(
            require_admin_actor(&context(Principal::User(user_id)), &mut missing_repo).await,
            |error| matches!(error, RequireAdminActorError::Forbidden),
        );

        let mut anonymous_repo = repository(None);
        assert_error(
            require_admin_actor(&context(Principal::Anonymous), &mut anonymous_repo).await,
            |error| matches!(error, RequireAdminActorError::AuthenticationRequired),
        );
    }

    #[tokio::test]
    async fn should_propagate_user_repository_error() {
        let user_id = UserId::new();
        let mut repo = repository(None);
        repo.error = Some(UserRepositoryError::Internal { source: boxed() });

        assert_error(
            require_admin_actor(&context(Principal::User(user_id)), &mut repo).await,
            |error| matches!(error, RequireAdminActorError::UserAdminRead(_)),
        );
    }
}
