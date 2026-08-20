use crate::ports::{AccessTokenStore, AccessTokenStoreError};
use application::operation_context::OperationContext;
use common::error::boxed::BoxError;
use common::user_id::UserId;
use std::collections::HashSet;
use user_core::access_token::{HashedRawAccessToken, Scope};

#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticateAccessTokenRequest {
    pub hashed_token: HashedRawAccessToken,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticateAccessTokenResult {
    pub user_id: UserId,
    pub scopes: HashSet<Scope>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthenticateAccessTokenError {
    #[error("access token not found")]
    NotFound,
    #[error("access token expired")]
    Expired,
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
pub trait AuthenticateAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthenticateAccessTokenRequest,
    ) -> Result<AuthenticateAccessTokenResult, AuthenticateAccessTokenError>;
}

pub struct AuthenticateAccessTokenHandler<S> {
    store: S,
}

impl<S> AuthenticateAccessTokenHandler<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl<S> AuthenticateAccessTokenUseCase for AuthenticateAccessTokenHandler<S>
where
    S: AccessTokenStore,
{
    #[tracing::instrument(
        name = "authenticate_access_token",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthenticateAccessTokenRequest,
    ) -> Result<AuthenticateAccessTokenResult, AuthenticateAccessTokenError> {
        let access_token = self
            .store
            .find_by_hashed_token(&request.hashed_token)
            .await?
            .ok_or(AuthenticateAccessTokenError::NotFound)?;

        if access_token.is_expired_at(time::OffsetDateTime::now_utc()) {
            return Err(AuthenticateAccessTokenError::Expired);
        }
        Ok(AuthenticateAccessTokenResult {
            user_id: access_token.user_id(),
            scopes: access_token.scopes().clone(),
        })
    }
}

impl From<AccessTokenStoreError> for AuthenticateAccessTokenError {
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
        AuthenticateAccessTokenError, AuthenticateAccessTokenHandler,
        AuthenticateAccessTokenRequest, AuthenticateAccessTokenUseCase,
    };
    use common::user_id::UserId;

    use crate::ports::{AccessTokenStore, AccessTokenStoreError};
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use common::error::boxed::{BoxError, box_error};
    use common::patch_field::PatchField;
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
        user_id: common::user_id::UserId,
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
            _user_id: &common::user_id::UserId,
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
            _user_id: &common::user_id::UserId,
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
            _user_id: &common::user_id::UserId,
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
    async fn should_authenticate_access_token_when_valid() {
        let user_id = UserId::new();
        let scopes = HashSet::from([Scope::ShopsWrite]);
        let valid = token(
            user_id,
            scopes.clone(),
            Some(OffsetDateTime::now_utc() + Duration::days(1)),
        );
        let hashed = valid.hashed_token().clone();
        let store = FakeAccessTokenStore::default();
        lock(&store.state).token = Some(valid);

        let result = assert_ok(
            AuthenticateAccessTokenHandler::new(store)
                .execute(
                    &ctx(Principal::Anonymous),
                    AuthenticateAccessTokenRequest {
                        hashed_token: hashed,
                    },
                )
                .await,
        );

        assert_eq!(user_id, result.user_id);
        assert_eq!(scopes, result.scopes);
    }

    #[tokio::test]
    async fn should_handle_access_token_auth_failures() {
        let user_id = UserId::new();
        let store = FakeAccessTokenStore::default();
        assert_error(
            AuthenticateAccessTokenHandler::new(store.clone())
                .execute(
                    &ctx(Principal::System),
                    AuthenticateAccessTokenRequest {
                        hashed_token: RawAccessToken::new().into(),
                    },
                )
                .await,
            |error| matches!(error, AuthenticateAccessTokenError::NotFound),
        );

        let expired = token(
            user_id,
            HashSet::from([Scope::ShopsWrite]),
            Some(OffsetDateTime::now_utc() - Duration::days(1)),
        );
        let hashed = expired.hashed_token().clone();
        lock(&store.state).token = Some(expired);
        assert_error(
            AuthenticateAccessTokenHandler::new(store.clone())
                .execute(
                    &ctx(Principal::System),
                    AuthenticateAccessTokenRequest {
                        hashed_token: hashed,
                    },
                )
                .await,
            |error| matches!(error, AuthenticateAccessTokenError::Expired),
        );
    }

    #[tokio::test]
    async fn should_map_access_token_store_errors_for_authenticate() {
        for kind in [
            StoreErrorKind::Conflict,
            StoreErrorKind::TemporarilyUnavailable,
            StoreErrorKind::InvalidPersistedState,
            StoreErrorKind::Internal,
        ] {
            let store = FakeAccessTokenStore::default();
            lock(&store.state).find_by_hashed_error = Some(kind);
            assert_error(
                AuthenticateAccessTokenHandler::new(store)
                    .execute(
                        &ctx(Principal::System),
                        AuthenticateAccessTokenRequest {
                            hashed_token: RawAccessToken::new().into(),
                        },
                    )
                    .await,
                |error| {
                    matches!(
                        error,
                        AuthenticateAccessTokenError::Conflict { .. }
                            | AuthenticateAccessTokenError::TemporarilyUnavailable { .. }
                            | AuthenticateAccessTokenError::InvalidPersistedState { .. }
                            | AuthenticateAccessTokenError::Internal { .. }
                    )
                },
            );
        }
    }
}
