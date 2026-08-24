use crate::ports::{AccessTokenDetails, AccessTokenDetailsReadError, AccessTokenDetailsReader};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessTokenId, AccessTokenName, AccessTokenOrigin, Scope};
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

pub struct GetAccessTokenHandler<R> {
    reader: R,
}

impl<R> GetAccessTokenHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R> GetAccessTokenUseCase for GetAccessTokenHandler<R>
where
    R: AccessTokenDetailsReader,
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
            .reader
            .find_by_id(request.user_id, request.access_token_id)
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

impl From<AccessTokenDetails> for AccessTokenView {
    fn from(access_token: AccessTokenDetails) -> Self {
        Self {
            user_id: access_token.user_id,
            access_token_id: access_token.access_token_id,
            name: access_token.name,
            scopes: access_token.scopes,
            origin: access_token.origin,
            expires: access_token.expires,
        }
    }
}

impl From<user_core::access_token::AccessToken> for AccessTokenView {
    fn from(access_token: user_core::access_token::AccessToken) -> Self {
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

impl From<AccessTokenDetailsReadError> for GetAccessTokenError {
    fn from(error: AccessTokenDetailsReadError) -> Self {
        match error {
            AccessTokenDetailsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenDetailsReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            AccessTokenDetailsReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GetAccessTokenError, GetAccessTokenHandler, GetAccessTokenRequest, GetAccessTokenUseCase,
    };
    use crate::ports::{AccessTokenDetails, AccessTokenDetailsReadError, AccessTokenDetailsReader};
    use application::error::box_error;
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::access_token::{AccessTokenId, AccessTokenName, AccessTokenOrigin};
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        details: Option<AccessTokenDetails>,
        unavailable: bool,
        calls: usize,
    }

    #[derive(Clone, Default)]
    struct FakeDetailsReader(Arc<Mutex<State>>);

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
    impl AccessTokenDetailsReader for FakeDetailsReader {
        async fn find_by_id(
            &self,
            _user_id: UserId,
            _access_token_id: AccessTokenId,
        ) -> Result<Option<AccessTokenDetails>, AccessTokenDetailsReadError> {
            let mut state = lock(&self.0);
            state.calls += 1;
            if state.unavailable {
                Err(AccessTokenDetailsReadError::TemporarilyUnavailable {
                    source: box_error(std::io::Error::other("unavailable")),
                })
            } else {
                Ok(state.details.clone())
            }
        }
    }

    #[tokio::test]
    async fn should_get_access_token_details_for_owner() {
        let user_id = UserId::new();
        let access_token_id = AccessTokenId::new();
        let reader = FakeDetailsReader::default();
        lock(&reader.0).details = Some(AccessTokenDetails {
            user_id,
            access_token_id,
            name: AccessTokenName::from("test token"),
            scopes: HashSet::new(),
            origin: AccessTokenOrigin::User,
            expires: None,
        });

        let result = GetAccessTokenHandler::new(reader.clone())
            .execute(
                &context(Principal::User(user_id)),
                GetAccessTokenRequest {
                    user_id,
                    access_token_id,
                },
            )
            .await;

        match result {
            Ok(view) => assert_eq!(access_token_id, view.access_token_id),
            Err(error) => panic!("expected access token details: {error:?}"),
        }
        assert_eq!(1, lock(&reader.0).calls);
    }

    #[tokio::test]
    async fn should_map_missing_and_unavailable_access_token_details() {
        let user_id = UserId::new();
        let access_token_id = AccessTokenId::new();
        let reader = FakeDetailsReader::default();
        let result = GetAccessTokenHandler::new(reader.clone())
            .execute(
                &context(Principal::System),
                GetAccessTokenRequest {
                    user_id,
                    access_token_id,
                },
            )
            .await;
        assert!(matches!(result, Err(GetAccessTokenError::NotFound)));

        lock(&reader.0).unavailable = true;
        let result = GetAccessTokenHandler::new(reader)
            .execute(
                &context(Principal::System),
                GetAccessTokenRequest {
                    user_id,
                    access_token_id,
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(GetAccessTokenError::TemporarilyUnavailable { .. })
        ));
    }
}
