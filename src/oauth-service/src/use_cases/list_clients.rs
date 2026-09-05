use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientListReader, OAuthClientView};
use crate::use_cases::support::authorize_oauth_client_read;
use application::operation_context::{OperationContext, Principal};
use application::pagination::{Cursor, CursoredResult};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::OAuthClientSearch;
use time::OffsetDateTime;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

const MAX_CURSOR_SIZE: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OAuthClientSearchCursor {
    pub position: OffsetDateTime,
    pub client_id: OAuthClientId,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListOAuthClientsRequest {
    pub search: OAuthClientSearch,
    pub cursor: Option<Cursor<OAuthClientSearchCursor>>,
}

pub type ListOAuthClientsResult = CursoredResult<OAuthClientView, OAuthClientSearchCursor>;

#[async_trait::async_trait]
pub trait ListOAuthClientsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListOAuthClientsRequest,
    ) -> Result<ListOAuthClientsResult, OAuthServiceError>;
}

pub struct ListOAuthClientsHandler<R, A> {
    reader: R,
    check_user_admin: A,
}

impl<R, A> ListOAuthClientsHandler<R, A> {
    pub fn new(reader: R, check_user_admin: A) -> Self {
        Self {
            reader,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<R, A> ListOAuthClientsUseCase for ListOAuthClientsHandler<R, A>
where
    R: OAuthClientListReader,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "list_oauth_clients",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListOAuthClientsRequest,
    ) -> Result<ListOAuthClientsResult, OAuthServiceError> {
        authorize_oauth_client_read(context)?;
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }
        authorize_oauth_client_admin(context, &self.check_user_admin).await?;

        let request = clamp_request_cursor(request);
        let mut result = self.reader.search(&request).await?;
        result.cursor.size = result.cursor.size.clamp(1, MAX_CURSOR_SIZE);
        tracing::Span::current().record("result_count", result.items.len());
        Ok(result)
    }
}

async fn authorize_oauth_client_admin<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), OAuthServiceError>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => check_user_admin
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(OAuthServiceError::from),
        Principal::Anonymous => Err(OAuthServiceError::AuthenticatedActorRequired),
    }
}

fn clamp_request_cursor(mut request: ListOAuthClientsRequest) -> ListOAuthClientsRequest {
    if let Some(cursor) = request.cursor.as_mut() {
        cursor.size = cursor.size.clamp(1, MAX_CURSOR_SIZE);
    }
    request
}

impl From<CheckUserAdminError> for OAuthServiceError {
    fn from(error: CheckUserAdminError) -> Self {
        match error {
            CheckUserAdminError::AuthenticatedActorRequired => Self::AuthenticatedActorRequired,
            CheckUserAdminError::Forbidden => Self::Forbidden,
            CheckUserAdminError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            CheckUserAdminError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            CheckUserAdminError::Internal { source } => Self::Internal { source },
            CheckUserAdminError::BeginTransactionFailed => Self::TemporarilyUnavailable {
                source: application::error::static_error(
                    "check user admin transaction begin failed",
                ),
            },
            CheckUserAdminError::CommitTransactionFailed => Self::TemporarilyUnavailable {
                source: application::error::static_error(
                    "check user admin transaction commit failed",
                ),
            },
        }
    }
}
