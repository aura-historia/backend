use crate::ports::{AccessTokenListReadError, AccessTokenListReader};
use crate::use_cases::queries::get_access_token::AccessTokenView;
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ListAccessTokensRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListAccessTokensResult {
    pub items: Vec<AccessTokenView>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListAccessTokensError {
    #[error("authenticated actor required to list access tokens")]
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
pub trait ListAccessTokensUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAccessTokensRequest,
    ) -> Result<ListAccessTokensResult, ListAccessTokensError>;
}

pub struct ListAccessTokensHandler<R> {
    reader: R,
}

impl<R> ListAccessTokensHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R> ListAccessTokensUseCase for ListAccessTokensHandler<R>
where
    R: AccessTokenListReader,
{
    #[tracing::instrument(
        name = "list_access_tokens",
        skip_all,
        fields(
            user_id = %request.user_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAccessTokensRequest,
    ) -> Result<ListAccessTokensResult, ListAccessTokensError> {
        authorize_access_token_read(context, request.user_id)?;
        let items = self
            .reader
            .list_for_user(request.user_id)
            .await?
            .into_iter()
            .map(AccessTokenView::from)
            .collect();

        Ok(ListAccessTokensResult { items })
    }
}

fn authorize_access_token_read(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), ListAccessTokensError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensRead)
        .user(&user_id)
        .service_or_system()
        .authorize::<ListAccessTokensError>()
}

impl From<OperationAuthorizationError> for ListAccessTokensError {
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

impl From<AccessTokenListReadError> for ListAccessTokensError {
    fn from(error: AccessTokenListReadError) -> Self {
        match error {
            AccessTokenListReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenListReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            AccessTokenListReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ListAccessTokensError, ListAccessTokensHandler, ListAccessTokensRequest,
        ListAccessTokensUseCase,
    };
    use crate::ports::{AccessTokenDetails, AccessTokenListReadError, AccessTokenListReader};
    use application::error::box_error;
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::access_token::{AccessTokenId, AccessTokenName, AccessTokenOrigin};
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        items: Vec<AccessTokenDetails>,
        unavailable: bool,
        calls: usize,
    }

    #[derive(Clone, Default)]
    struct FakeListReader(Arc<Mutex<State>>);

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
    impl AccessTokenListReader for FakeListReader {
        async fn list_for_user(
            &self,
            _user_id: UserId,
        ) -> Result<Vec<AccessTokenDetails>, AccessTokenListReadError> {
            let mut state = lock(&self.0);
            state.calls += 1;
            if state.unavailable {
                Err(AccessTokenListReadError::TemporarilyUnavailable {
                    source: box_error(std::io::Error::other("unavailable")),
                })
            } else {
                Ok(state.items.clone())
            }
        }
    }

    #[tokio::test]
    async fn should_list_access_tokens_for_owner() {
        let user_id = UserId::new();
        let reader = FakeListReader::default();
        lock(&reader.0).items.push(AccessTokenDetails {
            user_id,
            access_token_id: AccessTokenId::new(),
            name: AccessTokenName::from("test token"),
            scopes: HashSet::new(),
            origin: AccessTokenOrigin::User,
            expires: None,
        });

        let result = ListAccessTokensHandler::new(reader.clone())
            .execute(
                &context(Principal::User(user_id)),
                ListAccessTokensRequest { user_id },
            )
            .await;

        match result {
            Ok(result) => assert_eq!(1, result.items.len()),
            Err(error) => panic!("expected access token list: {error:?}"),
        }
        assert_eq!(1, lock(&reader.0).calls);
    }

    #[tokio::test]
    async fn should_map_unavailable_access_token_list() {
        let user_id = UserId::new();
        let reader = FakeListReader::default();
        lock(&reader.0).unavailable = true;

        let result = ListAccessTokensHandler::new(reader)
            .execute(
                &context(Principal::System),
                ListAccessTokensRequest { user_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(ListAccessTokensError::TemporarilyUnavailable { .. })
        ));
    }
}
