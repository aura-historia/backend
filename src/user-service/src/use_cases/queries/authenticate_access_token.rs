use crate::ports::{AccessTokenAuthenticationReadError, AccessTokenAuthenticationReader};
use application::error::BoxError;
use application::operation_context::OperationContext;
use std::collections::HashSet;
use user_core::access_token::{HashedRawAccessToken, Scope};
use user_core::user_id::UserId;

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

pub struct AuthenticateAccessTokenHandler<R> {
    reader: R,
}

impl<R> AuthenticateAccessTokenHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R> AuthenticateAccessTokenUseCase for AuthenticateAccessTokenHandler<R>
where
    R: AccessTokenAuthenticationReader,
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
            .reader
            .find_authentication_by_hashed_token(&request.hashed_token)
            .await?
            .ok_or(AuthenticateAccessTokenError::NotFound)?;

        if access_token
            .expires
            .is_some_and(|expires| expires < time::OffsetDateTime::now_utc())
        {
            return Err(AuthenticateAccessTokenError::Expired);
        }
        Ok(AuthenticateAccessTokenResult {
            user_id: access_token.user_id,
            scopes: access_token.scopes,
        })
    }
}

impl From<AccessTokenAuthenticationReadError> for AuthenticateAccessTokenError {
    fn from(error: AccessTokenAuthenticationReadError) -> Self {
        match error {
            AccessTokenAuthenticationReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenAuthenticationReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            AccessTokenAuthenticationReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthenticateAccessTokenError, AuthenticateAccessTokenHandler,
        AuthenticateAccessTokenRequest, AuthenticateAccessTokenUseCase,
    };
    use crate::ports::{
        AccessTokenAuthentication, AccessTokenAuthenticationReadError,
        AccessTokenAuthenticationReader,
    };
    use application::error::box_error;
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::{Duration, OffsetDateTime};
    use user_core::access_token::{
        AccessTokenId, AccessTokenOrigin, HashedRawAccessToken, RawAccessToken, Scope,
    };
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        authentication: Option<AccessTokenAuthentication>,
        unavailable: bool,
        calls: usize,
    }

    #[derive(Clone, Default)]
    struct FakeAuthenticationReader(Arc<Mutex<State>>);

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    #[async_trait::async_trait]
    impl AccessTokenAuthenticationReader for FakeAuthenticationReader {
        async fn find_authentication_by_hashed_token(
            &self,
            _hashed_token: &HashedRawAccessToken,
        ) -> Result<Option<AccessTokenAuthentication>, AccessTokenAuthenticationReadError> {
            let mut state = lock(&self.0);
            state.calls += 1;
            if state.unavailable {
                Err(AccessTokenAuthenticationReadError::TemporarilyUnavailable {
                    source: box_error(std::io::Error::other("unavailable")),
                })
            } else {
                Ok(state.authentication.clone())
            }
        }
    }

    #[tokio::test]
    async fn should_authenticate_unexpired_access_token() {
        let user_id = UserId::new();
        let scopes = HashSet::from([Scope::ProductListingsWrite]);
        let reader = FakeAuthenticationReader::default();
        lock(&reader.0).authentication = Some(AccessTokenAuthentication {
            access_token_id: AccessTokenId::new(),
            user_id,
            scopes: scopes.clone(),
            origin: AccessTokenOrigin::User,
            expires: Some(OffsetDateTime::now_utc() + Duration::days(1)),
        });

        let result = AuthenticateAccessTokenHandler::new(reader.clone())
            .execute(
                &context(),
                AuthenticateAccessTokenRequest {
                    hashed_token: RawAccessToken::new().into(),
                },
            )
            .await;

        match result {
            Ok(result) => {
                assert_eq!(user_id, result.user_id);
                assert_eq!(scopes, result.scopes);
            }
            Err(error) => panic!("expected authentication success: {error:?}"),
        }
        assert_eq!(1, lock(&reader.0).calls);
    }

    #[tokio::test]
    async fn should_reject_expired_and_unavailable_access_token_reads() {
        let reader = FakeAuthenticationReader::default();
        lock(&reader.0).authentication = Some(AccessTokenAuthentication {
            access_token_id: AccessTokenId::new(),
            user_id: UserId::new(),
            scopes: HashSet::new(),
            origin: AccessTokenOrigin::User,
            expires: Some(OffsetDateTime::now_utc() - Duration::days(1)),
        });
        let result = AuthenticateAccessTokenHandler::new(reader.clone())
            .execute(
                &context(),
                AuthenticateAccessTokenRequest {
                    hashed_token: RawAccessToken::new().into(),
                },
            )
            .await;
        assert!(matches!(result, Err(AuthenticateAccessTokenError::Expired)));

        lock(&reader.0).unavailable = true;
        let result = AuthenticateAccessTokenHandler::new(reader)
            .execute(
                &context(),
                AuthenticateAccessTokenRequest {
                    hashed_token: RawAccessToken::new().into(),
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(AuthenticateAccessTokenError::TemporarilyUnavailable { .. })
        ));
    }
}
