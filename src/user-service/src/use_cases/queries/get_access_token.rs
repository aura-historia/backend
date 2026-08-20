use crate::ports::{AccessTokenStore, AccessTokenStoreError};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, Scope,
};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct GetAccessTokenRequest {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccessTokenView {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
    pub name: AccessTokenName,
    pub scopes: HashSet<Scope>,
    pub origin: AccessTokenOrigin,
    pub expires: Option<OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub enum GetAccessTokenError {
    #[error("authenticated actor required to get access token")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("access token not found")]
    NotFound,
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
pub trait GetAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAccessTokenRequest,
    ) -> Result<AccessTokenView, GetAccessTokenError>;
}

pub struct GetAccessTokenHandler<S> {
    store: S,
}

impl<S> GetAccessTokenHandler<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl<S> GetAccessTokenUseCase for GetAccessTokenHandler<S>
where
    S: AccessTokenStore,
{
    #[tracing::instrument(
        name = "get_access_token",
        skip_all,
        fields(
            user_id = %request.user_id,
            access_token_id = %request.access_token_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAccessTokenRequest,
    ) -> Result<AccessTokenView, GetAccessTokenError> {
        authorize_access_token_read(context, request.user_id)?;
        let access_token = self
            .store
            .find_by_id(&request.user_id, &request.access_token_id)
            .await?
            .ok_or(GetAccessTokenError::NotFound)?;

        Ok(AccessTokenView::from(access_token))
    }
}

impl From<OperationAuthorizationError> for GetAccessTokenError {
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

impl From<AccessToken> for AccessTokenView {
    fn from(access_token: AccessToken) -> Self {
        Self {
            user_id: access_token.user_id(),
            access_token_id: access_token.id(),
            name: access_token.name().clone(),
            scopes: access_token.scopes().clone(),
            origin: access_token.origin().clone(),
            expires: access_token.expires(),
        }
    }
}

fn authorize_access_token_read(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), GetAccessTokenError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensRead)
        .user(&user_id)
        .service_or_system()
        .authorize::<GetAccessTokenError>()
}

impl From<AccessTokenStoreError> for GetAccessTokenError {
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
        GetAccessTokenError, GetAccessTokenHandler, GetAccessTokenRequest, GetAccessTokenUseCase,
    };
    use user_core::user_id::UserId;

    use crate::ports::{AccessTokenStore, AccessTokenStoreError};
    use application::error::{BoxError, box_error};
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use application::patch_field::PatchField;
    use std::collections::HashSet;
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
    async fn should_get_access_token_when_found() {
        let user_id = UserId::new();
        let current = token(user_id, HashSet::new(), None);
        let store = FakeAccessTokenStore::default();
        lock(&store.state).token = Some(current.clone());

        assert_eq!(
            current.id(),
            assert_ok(
                GetAccessTokenHandler::new(store)
                    .execute(
                        &ctx(Principal::User(user_id)),
                        GetAccessTokenRequest {
                            user_id,
                            access_token_id: current.id(),
                        },
                    )
                    .await,
            )
            .access_token_id,
        );
    }

    #[tokio::test]
    async fn should_return_not_found_when_access_token_missing() {
        let user_id = UserId::new();
        assert_error(
            GetAccessTokenHandler::new(FakeAccessTokenStore::default())
                .execute(
                    &ctx(Principal::System),
                    GetAccessTokenRequest {
                        user_id,
                        access_token_id: AccessTokenId::new(),
                    },
                )
                .await,
            |error| matches!(error, GetAccessTokenError::NotFound),
        );
    }

    #[tokio::test]
    async fn should_map_access_token_store_errors_for_get() {
        for kind in [
            StoreErrorKind::Conflict,
            StoreErrorKind::TemporarilyUnavailable,
            StoreErrorKind::InvalidPersistedState,
            StoreErrorKind::Internal,
        ] {
            let user_id = UserId::new();
            let store = FakeAccessTokenStore::default();
            lock(&store.state).find_by_id_error = Some(kind);
            assert_error(
                GetAccessTokenHandler::new(store)
                    .execute(
                        &ctx(Principal::System),
                        GetAccessTokenRequest {
                            user_id,
                            access_token_id: AccessTokenId::new(),
                        },
                    )
                    .await,
                |error| {
                    matches!(
                        error,
                        GetAccessTokenError::Conflict { .. }
                            | GetAccessTokenError::TemporarilyUnavailable { .. }
                            | GetAccessTokenError::InvalidPersistedState { .. }
                            | GetAccessTokenError::Internal { .. }
                    )
                },
            );
        }
    }
}
