use crate::ports::{
    AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
};
use application::error::BoxError;
use application::operation_context::{
    AuthenticationRequired, CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, NewAccessToken, RawAccessToken,
    Scope,
};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateAccessTokenCommand {
    pub user_id: UserId,
    pub name: AccessTokenName,
    pub scopes: HashSet<Scope>,
    pub expires: Option<OffsetDateTime>,
    pub origin: AccessTokenOrigin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateAccessTokenResult {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
    pub raw_access_token: RawAccessToken,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateAccessTokenError {
    #[error("authenticated actor required to create access token")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("delegated credentials cannot issue access tokens")]
    DelegatedIssuanceForbidden,
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
    #[error("failed to begin create access token transaction")]
    BeginTransactionFailed(#[source] application::error::BoxError),
    #[error("failed to commit create access token transaction")]
    CommitTransactionFailed(#[source] application::error::BoxError),
}

#[async_trait::async_trait]
pub trait CreateAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateAccessTokenCommand,
    ) -> Result<CreateAccessTokenResult, CreateAccessTokenError>;
}

pub struct CreateAccessTokenHandler<U, R> {
    unit_of_work: U,
    repository: R,
}

impl<U, R> CreateAccessTokenHandler<U, R> {
    pub fn new(unit_of_work: U, repository: R) -> Self {
        Self {
            unit_of_work,
            repository,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> CreateAccessTokenUseCase for CreateAccessTokenHandler<U, R>
where
    U: UnitOfWork,
    R: AccessTokenRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "create_access_token",
        skip_all,
        fields(
            user_id = %command.user_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateAccessTokenCommand,
    ) -> Result<CreateAccessTokenResult, CreateAccessTokenError> {
        authorize_access_token_write(context, command.user_id)?;
        let principal = context.principal.require_authenticated()?;
        tracing::Span::current().record("actor_id", tracing::field::display(principal.label()));

        let raw_access_token = RawAccessToken::new();
        let access_token = AccessToken::create(NewAccessToken {
            id: AccessTokenId::new(),
            hashed_token: raw_access_token.clone().into(),
            user_id: command.user_id,
            name: command.name,
            scopes: command.scopes,
            origin: command.origin,
            expires: command.expires,
        });

        let mut tx =
            self.unit_of_work.begin().await.map_err(|source| {
                CreateAccessTokenError::BeginTransactionFailed(Box::new(source))
            })?;
        self.repository
            .in_transaction(&mut tx)
            .insert(&access_token)
            .await?;
        tx.commit()
            .await
            .map_err(|source| CreateAccessTokenError::CommitTransactionFailed(Box::new(source)))?;

        tracing::info!(
            event = "access_token.created",
            actor_id = %principal.label(),
            user_id = %access_token.user_id(),
            access_token_id = %access_token.id(),
            outcome = "success",
        );

        Ok(CreateAccessTokenResult {
            user_id: access_token.user_id(),
            access_token_id: access_token.id(),
            raw_access_token,
        })
    }
}

impl From<OperationAuthorizationError> for CreateAccessTokenError {
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

impl From<AuthenticationRequired> for CreateAccessTokenError {
    fn from(_: AuthenticationRequired) -> Self {
        Self::AuthenticatedActorRequired
    }
}

/// Issuance requires first-party authority. The operation context does not
/// carry a delegation's lifetime, so a delegate could not be prevented from
/// minting a token that outlives its own grant.
fn authorize_access_token_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), CreateAccessTokenError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<CreateAccessTokenError>()?;
    if context.principal.is_delegated() {
        return Err(CreateAccessTokenError::DelegatedIssuanceForbidden);
    }
    Ok(())
}

impl From<AccessTokenRepositoryError> for CreateAccessTokenError {
    fn from(error: AccessTokenRepositoryError) -> Self {
        match error {
            AccessTokenRepositoryError::ConcurrencyConflict => Self::Internal {
                source: application::error::box_error(std::io::Error::other(
                    "unexpected access token concurrency conflict during creation",
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
        CreateAccessTokenCommand, CreateAccessTokenError, CreateAccessTokenHandler,
        CreateAccessTokenUseCase,
    };
    use crate::ports::{
        AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
        AccessTokenStorageVersion, VersionedAccessToken,
    };
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use domain_primitives::versioned::Versioned;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::access_token::{
        AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, HashedRawAccessToken, Scope,
    };
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        token: Option<AccessToken>,
        begins: usize,
        commits: usize,
        insert_calls: usize,
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

    fn versioned(token: AccessToken) -> VersionedAccessToken {
        Versioned::new(token, AccessTokenStorageVersion::INITIAL)
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
            token: &AccessToken,
        ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
            let mut state = lock(&self.0.0);
            state.insert_calls += 1;
            state.token = Some(token.clone());
            Ok(versioned(token.clone()))
        }

        async fn update(
            &mut self,
            token: &AccessToken,
            _expected_version: AccessTokenStorageVersion,
        ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
            Ok(versioned(token.clone()))
        }

        async fn delete_by_id(
            &mut self,
            _user_id: UserId,
            _access_token_id: AccessTokenId,
        ) -> Result<bool, AccessTokenRepositoryError> {
            Ok(true)
        }

        async fn delete_by_user_id(
            &mut self,
            _user_id: UserId,
        ) -> Result<u64, AccessTokenRepositoryError> {
            Ok(0)
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
    async fn should_create_access_token_in_committed_transaction() {
        let user_id = UserId::new();
        let fakes = Fakes::default();
        let result = CreateAccessTokenHandler::new(fakes.clone(), fakes.clone())
            .execute(
                &context(Principal::User(user_id)),
                CreateAccessTokenCommand {
                    user_id,
                    name: AccessTokenName::from("created"),
                    scopes: HashSet::from([Scope::ProductListingsWrite]),
                    expires: None,
                    origin: AccessTokenOrigin::User,
                },
            )
            .await;

        match result {
            Ok(result) => assert_eq!(user_id, result.user_id),
            Err(error) => panic!("expected access token creation: {error:?}"),
        }
        let state = lock(&fakes.0);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.insert_calls);
        assert_eq!(1, state.commits);
        assert_eq!(
            Some(user_id),
            state.token.as_ref().map(AccessToken::user_id)
        );
    }

    #[tokio::test]
    async fn should_reject_anonymous_create_before_starting_transaction() {
        let fakes = Fakes::default();
        let result = CreateAccessTokenHandler::new(fakes.clone(), fakes.clone())
            .execute(
                &context(Principal::Anonymous),
                CreateAccessTokenCommand {
                    user_id: UserId::new(),
                    name: AccessTokenName::from("created"),
                    scopes: HashSet::new(),
                    expires: None,
                    origin: AccessTokenOrigin::User,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(CreateAccessTokenError::AuthenticatedActorRequired)
        ));
        assert_eq!(0, lock(&fakes.0).begins);
    }

    fn command(user_id: UserId, scopes: HashSet<Scope>) -> CreateAccessTokenCommand {
        CreateAccessTokenCommand {
            user_id,
            name: AccessTokenName::from("created"),
            scopes,
            expires: None,
            origin: AccessTokenOrigin::User,
        }
    }

    fn delegated(user_id: UserId, capabilities: &[Scope]) -> Principal {
        Principal::DelegatedUser {
            user_id,
            capabilities: capabilities.iter().copied().collect(),
        }
    }

    #[tokio::test]
    async fn should_reject_delegated_issuance_without_issuing_or_storing_token() {
        let user_id = UserId::new();
        let delegate_scopes = [Scope::AccessTokensWrite, Scope::ProductListingsWrite];

        for scopes in [
            HashSet::new(),
            HashSet::from([Scope::ProductListingsWrite]),
            HashSet::from(delegate_scopes),
            HashSet::from([Scope::UsersWrite]),
        ] {
            let fakes = Fakes::default();
            let result = CreateAccessTokenHandler::new(fakes.clone(), fakes.clone())
                .execute(
                    &context(delegated(user_id, &delegate_scopes)),
                    command(user_id, scopes),
                )
                .await;

            assert!(
                matches!(
                    result,
                    Err(CreateAccessTokenError::DelegatedIssuanceForbidden)
                ),
                "unexpected result: {result:?}"
            );
            let state = lock(&fakes.0);
            assert_eq!(0, state.begins);
            assert_eq!(0, state.insert_calls);
            assert!(state.token.is_none());
        }
    }

    #[tokio::test]
    async fn should_keep_capability_and_ownership_checks_for_delegates() {
        let user_id = UserId::new();

        for (principal, target) in [
            (delegated(user_id, &[]), user_id),
            (
                delegated(user_id, &[Scope::AccessTokensWrite]),
                UserId::new(),
            ),
            (Principal::User(user_id), UserId::new()),
        ] {
            let fakes = Fakes::default();
            let result = CreateAccessTokenHandler::new(fakes.clone(), fakes.clone())
                .execute(&context(principal), command(target, HashSet::new()))
                .await;

            assert!(
                matches!(result, Err(CreateAccessTokenError::Forbidden)),
                "unexpected result: {result:?}"
            );
            assert_eq!(0, lock(&fakes.0).begins);
        }
    }

    #[tokio::test]
    async fn should_let_service_and_system_issue_tokens_for_users() {
        let user_id = UserId::new();

        for principal in [Principal::Service("svc".to_owned()), Principal::System] {
            let fakes = Fakes::default();
            let result = CreateAccessTokenHandler::new(fakes.clone(), fakes.clone())
                .execute(
                    &context(principal),
                    command(user_id, HashSet::from([Scope::UsersWrite])),
                )
                .await;

            assert!(result.is_ok(), "unexpected result: {result:?}");
            assert_eq!(1, lock(&fakes.0).insert_calls);
        }
    }
}
