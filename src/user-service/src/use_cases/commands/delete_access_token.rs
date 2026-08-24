use crate::ports::{
    AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use user_core::access_token::AccessTokenId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteAccessTokenCommand {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteAccessTokenResult {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteAccessTokenError {
    #[error("authenticated actor required to delete access token")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("access token already exists")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary access token store failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted access token state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal access token persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin delete access token transaction")]
    BeginTransactionFailed,
    #[error("failed to commit delete access token transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait DeleteAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteAccessTokenCommand,
    ) -> Result<DeleteAccessTokenResult, DeleteAccessTokenError>;
}

pub struct DeleteAccessTokenHandler<U, R> {
    unit_of_work: U,
    repository: R,
}

impl<U, R> DeleteAccessTokenHandler<U, R> {
    pub fn new(unit_of_work: U, repository: R) -> Self {
        Self {
            unit_of_work,
            repository,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> DeleteAccessTokenUseCase for DeleteAccessTokenHandler<U, R>
where
    U: UnitOfWork,
    R: AccessTokenRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "delete_access_token",
        skip_all,
        fields(
            user_id = %command.user_id,
            access_token_id = %command.access_token_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteAccessTokenCommand,
    ) -> Result<DeleteAccessTokenResult, DeleteAccessTokenError> {
        authorize_access_token_write(context, command.user_id)?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| DeleteAccessTokenError::BeginTransactionFailed)?;
        self.repository
            .in_transaction(&mut tx)
            .delete_by_id(command.user_id, command.access_token_id)
            .await?;
        tx.commit()
            .await
            .map_err(|_| DeleteAccessTokenError::CommitTransactionFailed)?;

        tracing::info!(
            event = "access_token.deleted",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %command.user_id,
            access_token_id = %command.access_token_id,
            outcome = "success",
        );

        Ok(DeleteAccessTokenResult {
            user_id: command.user_id,
            access_token_id: command.access_token_id,
        })
    }
}

fn authorize_access_token_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), DeleteAccessTokenError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<DeleteAccessTokenError>()
}

impl From<OperationAuthorizationError> for DeleteAccessTokenError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<AccessTokenRepositoryError> for DeleteAccessTokenError {
    fn from(error: AccessTokenRepositoryError) -> Self {
        match error {
            AccessTokenRepositoryError::ConcurrencyConflict => Self::Internal {
                source: application::error::box_error(std::io::Error::other(
                    "unexpected access token concurrency conflict during deletion",
                )),
            },
            AccessTokenRepositoryError::Conflict { source } => Self::Conflict { source },
            AccessTokenRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AccessTokenRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeleteAccessTokenCommand, DeleteAccessTokenError, DeleteAccessTokenHandler,
        DeleteAccessTokenUseCase,
    };
    use crate::ports::{
        AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
        AccessTokenStorageVersion, VersionedAccessToken,
    };
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::access_token::{AccessToken, AccessTokenId, HashedRawAccessToken};
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        delete_calls: usize,
    }

    #[derive(Clone, Default)]
    struct Fakes(Arc<Mutex<State>>);
    struct FakeTx(Fakes);
    struct FakeRepository(Fakes);

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0.0).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for Fakes {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(FakeTx(self.clone()))
        }
    }

    #[async_trait::async_trait]
    impl AccessTokenRepository for FakeRepository {
        async fn find_by_id(
            &mut self,
            _user_id: UserId,
            _access_token_id: AccessTokenId,
        ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError> {
            Ok(None)
        }

        async fn find_by_hashed_token(
            &mut self,
            _hashed_token: &HashedRawAccessToken,
        ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError> {
            Ok(None)
        }

        async fn insert(
            &mut self,
            _token: &AccessToken,
        ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
            Err(AccessTokenRepositoryError::Internal {
                source: application::error::box_error(std::io::Error::other("not used")),
            })
        }

        async fn update(
            &mut self,
            _token: &AccessToken,
            _expected_version: AccessTokenStorageVersion,
        ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
            Err(AccessTokenRepositoryError::Internal {
                source: application::error::box_error(std::io::Error::other("not used")),
            })
        }

        async fn delete_by_id(
            &mut self,
            _user_id: UserId,
            _access_token_id: AccessTokenId,
        ) -> Result<bool, AccessTokenRepositoryError> {
            lock(&self.0.0).delete_calls += 1;
            Ok(true)
        }
    }

    impl AccessTokenRepositoryFactory<FakeTx> for Fakes {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl AccessTokenRepository + 'tx {
            FakeRepository(self.clone())
        }
    }

    #[tokio::test]
    async fn should_delete_access_token_in_committed_transaction() {
        let user_id = UserId::new();
        let fakes = Fakes::default();
        let result = DeleteAccessTokenHandler::new(fakes.clone(), fakes.clone())
            .execute(
                &context(Principal::User(user_id)),
                DeleteAccessTokenCommand {
                    user_id,
                    access_token_id: AccessTokenId::new(),
                },
            )
            .await;

        assert!(result.is_ok());
        let state = lock(&fakes.0);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.delete_calls);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_delete_before_starting_transaction() {
        let fakes = Fakes::default();
        let result = DeleteAccessTokenHandler::new(fakes.clone(), fakes.clone())
            .execute(
                &context(Principal::Anonymous),
                DeleteAccessTokenCommand {
                    user_id: UserId::new(),
                    access_token_id: AccessTokenId::new(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(DeleteAccessTokenError::AuthenticatedActorRequired)
        ));
        assert_eq!(0, lock(&fakes.0).begins);
    }
}
