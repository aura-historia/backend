use crate::ports::{AccessTokenStore, AccessTokenStoreError};
use application::error::BoxError;
use application::operation_context::{
    AuthenticationRequired, CredentialCapability, OperationAuthorizationError, OperationContext,
};
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
    #[error("internal access token store failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait CreateAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateAccessTokenCommand,
    ) -> Result<CreateAccessTokenResult, CreateAccessTokenError>;
}

pub struct CreateAccessTokenHandler<S> {
    store: S,
}

impl<S> CreateAccessTokenHandler<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl<S> CreateAccessTokenUseCase for CreateAccessTokenHandler<S>
where
    S: AccessTokenStore,
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

        self.store.insert(access_token.clone()).await?;

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

fn authorize_access_token_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), CreateAccessTokenError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<CreateAccessTokenError>()
}

impl From<AccessTokenStoreError> for CreateAccessTokenError {
    fn from(error: AccessTokenStoreError) -> Self {
        match error {
            AccessTokenStoreError::Conflict { source } => Self::Conflict { source },
            AccessTokenStoreError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenStoreError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AccessTokenStoreError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code, unused_imports)]
    use super::{
        CreateAccessTokenCommand, CreateAccessTokenError, CreateAccessTokenHandler,
        CreateAccessTokenUseCase,
    };
    use user_core::user_id::UserId;

    use crate::ports::{AccessTokenStore, AccessTokenStoreError};
    use application::error::{BoxError, box_error};
    use application::operation_context::{
        CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
    };
    use application::patch_field::PatchField;
    use std::collections::{BTreeSet, HashSet};
    use std::fmt::Debug;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::{Duration, OffsetDateTime};
    use user_core::access_token::{
        AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, HashedRawAccessToken,
        NewAccessToken, RawAccessToken, Scope,
    };

    #[derive(Debug, Clone, Copy)]
    enum StoreErrorKind {
        Conflict,
        TemporarilyUnavailable,
        InvalidPersistedState,
        Internal,
    }

    #[derive(Default)]
    struct StoreState {
        token: Option<AccessToken>,
        tokens: Vec<AccessToken>,
        find_by_id_error: Option<StoreErrorKind>,
        find_by_hashed_error: Option<StoreErrorKind>,
        list_error: Option<StoreErrorKind>,
        insert_error: Option<StoreErrorKind>,
        replace_error: Option<StoreErrorKind>,
        delete_error: Option<StoreErrorKind>,
        find_by_id_calls: usize,
        find_by_hashed_calls: usize,
        list_calls: usize,
        insert_calls: usize,
        replace_calls: usize,
        delete_calls: usize,
    }

    #[derive(Clone, Default)]
    struct FakeAccessTokenStore {
        state: Arc<Mutex<StoreState>>,
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn ctx(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    fn token(
        user_id: user_core::user_id::UserId,
        scopes: HashSet<Scope>,
        expires: Option<OffsetDateTime>,
    ) -> AccessToken {
        let raw = RawAccessToken::new();
        AccessToken::create(NewAccessToken {
            id: AccessTokenId::new(),
            hashed_token: raw.into(),
            user_id,
            name: AccessTokenName::from("test token"),
            scopes,
            origin: AccessTokenOrigin::User,
            expires,
        })
    }

    fn boxed() -> BoxError {
        box_error(std::io::Error::other("boom"))
    }

    fn store_error(kind: StoreErrorKind) -> AccessTokenStoreError {
        match kind {
            StoreErrorKind::Conflict => AccessTokenStoreError::Conflict { source: boxed() },
            StoreErrorKind::TemporarilyUnavailable => {
                AccessTokenStoreError::TemporarilyUnavailable { source: boxed() }
            }
            StoreErrorKind::InvalidPersistedState => {
                AccessTokenStoreError::InvalidPersistedState { source: boxed() }
            }
            StoreErrorKind::Internal => AccessTokenStoreError::Internal { source: boxed() },
        }
    }

    fn assert_error<T, E, F>(result: Result<T, E>, predicate: F)
    where
        E: Debug,
        F: FnOnce(&E) -> bool,
    {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => assert!(predicate(&error), "unexpected error: {error:?}"),
        }
    }

    fn assert_ok<T, E: Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected ok, got {error:?}"),
        }
    }

    #[async_trait::async_trait]
    impl AccessTokenStore for FakeAccessTokenStore {
        async fn find_by_id(
            &self,
            _user_id: &user_core::user_id::UserId,
            _access_token_id: &AccessTokenId,
        ) -> Result<Option<AccessToken>, AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.find_by_id_calls += 1;
            if let Some(kind) = state.find_by_id_error {
                Err(store_error(kind))
            } else {
                Ok(state.token.clone())
            }
        }

        async fn find_by_hashed_token(
            &self,
            _hashed_token: &HashedRawAccessToken,
        ) -> Result<Option<AccessToken>, AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.find_by_hashed_calls += 1;
            if let Some(kind) = state.find_by_hashed_error {
                Err(store_error(kind))
            } else {
                Ok(state.token.clone())
            }
        }

        async fn list_for_user(
            &self,
            _user_id: &user_core::user_id::UserId,
        ) -> Result<Vec<AccessToken>, AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.list_calls += 1;
            if let Some(kind) = state.list_error {
                Err(store_error(kind))
            } else {
                Ok(state.tokens.clone())
            }
        }

        async fn insert(&self, access_token: AccessToken) -> Result<(), AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.insert_calls += 1;
            if let Some(kind) = state.insert_error {
                Err(store_error(kind))
            } else {
                state.token = Some(access_token);
                Ok(())
            }
        }

        async fn replace(&self, access_token: AccessToken) -> Result<(), AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.replace_calls += 1;
            if let Some(kind) = state.replace_error {
                Err(store_error(kind))
            } else {
                state.token = Some(access_token);
                Ok(())
            }
        }

        async fn delete(
            &self,
            _user_id: &user_core::user_id::UserId,
            _access_token_id: &AccessTokenId,
        ) -> Result<(), AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.delete_calls += 1;
            if let Some(kind) = state.delete_error {
                Err(store_error(kind))
            } else {
                state.token = None;
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn should_create_access_token_when_authenticated() {
        let user_id = UserId::new();
        let store = FakeAccessTokenStore::default();
        let scopes = HashSet::from([Scope::ShopsWrite]);
        let created = assert_ok(
            CreateAccessTokenHandler::new(store.clone())
                .execute(
                    &ctx(Principal::User(user_id)),
                    CreateAccessTokenCommand {
                        user_id,
                        name: AccessTokenName::from("created"),
                        scopes,
                        expires: None,
                        origin: AccessTokenOrigin::User,
                    },
                )
                .await,
        );

        assert_eq!(user_id, created.user_id);
        let state = lock(&store.state);
        assert_eq!(1, state.insert_calls);
        assert_eq!(
            Some(user_id),
            state.token.as_ref().map(|token| token.user_id())
        );
    }

    #[tokio::test]
    async fn should_create_access_token_with_delegated_user() {
        let user_id = UserId::new();
        let store = FakeAccessTokenStore::default();

        let created = assert_ok(
            CreateAccessTokenHandler::new(store.clone())
                .execute(
                    &ctx(Principal::DelegatedUser {
                        user_id,
                        capabilities: BTreeSet::from([CredentialCapability::AccessTokensWrite]),
                    }),
                    CreateAccessTokenCommand {
                        user_id,
                        name: AccessTokenName::from("delegated"),
                        scopes: HashSet::new(),
                        expires: None,
                        origin: AccessTokenOrigin::User,
                    },
                )
                .await,
        );

        assert_eq!(user_id, created.user_id);
        assert_eq!(1, lock(&store.state).insert_calls);
    }

    #[tokio::test]
    async fn should_not_create_access_token_when_anonymous() {
        let user_id = UserId::new();
        let store = FakeAccessTokenStore::default();
        assert_error(
            CreateAccessTokenHandler::new(store.clone())
                .execute(
                    &ctx(Principal::Anonymous),
                    CreateAccessTokenCommand {
                        user_id,
                        name: AccessTokenName::from("x"),
                        scopes: HashSet::new(),
                        expires: None,
                        origin: AccessTokenOrigin::User,
                    },
                )
                .await,
            |error| matches!(error, CreateAccessTokenError::AuthenticatedActorRequired),
        );
        assert_eq!(0, lock(&store.state).insert_calls);
    }

    #[tokio::test]
    async fn should_map_access_token_store_errors_for_create() {
        for kind in [
            StoreErrorKind::Conflict,
            StoreErrorKind::TemporarilyUnavailable,
            StoreErrorKind::InvalidPersistedState,
            StoreErrorKind::Internal,
        ] {
            let user_id = UserId::new();
            let store = FakeAccessTokenStore::default();
            lock(&store.state).insert_error = Some(kind);
            assert_error(
                CreateAccessTokenHandler::new(store)
                    .execute(
                        &ctx(Principal::System),
                        CreateAccessTokenCommand {
                            user_id,
                            name: AccessTokenName::from("x"),
                            scopes: HashSet::new(),
                            expires: None,
                            origin: AccessTokenOrigin::User,
                        },
                    )
                    .await,
                |error| {
                    matches!(
                        error,
                        CreateAccessTokenError::Conflict { .. }
                            | CreateAccessTokenError::TemporarilyUnavailable { .. }
                            | CreateAccessTokenError::InvalidPersistedState { .. }
                            | CreateAccessTokenError::Internal { .. }
                    )
                },
            );
        }
    }
}
