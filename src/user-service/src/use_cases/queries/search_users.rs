use crate::ports::{
    UserAdminReadError, UserAdminReaderFactory, UserSearchReadError, UserSearchReader,
    UserSearchReaderFactory,
};
use crate::use_cases::authorization::{
    RequireAdminActorError, require_admin_actor, require_admin_actor_credential,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::operation_context::{CredentialCapability, OperationContext, Principal};
use common::pagination::cursor::Cursor;
use common::sort::Sort;
use common::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use serde_email::Email;
use user_core::{role::UserRole, tier::UserTier, user_search::UserSearch};

#[derive(Debug, Clone, PartialEq)]
pub struct SearchUsersRequest {
    pub search: UserSearch,
    pub sort: Option<Sort<user_core::sort_user_field::SortUserField>>,
    pub cursor: Option<Cursor<UserId>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserSummary {
    pub user_id: UserId,
    pub email: Email,
    pub first_name: Option<user_core::first_name::FirstName>,
    pub last_name: Option<user_core::last_name::LastName>,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: Option<StripeCustomerId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchUsersResult {
    pub items: Vec<UserSummary>,
    pub cursor: Cursor<UserId>,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchUsersError {
    #[error("authenticated actor required to search users")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary user search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search users transaction")]
    BeginTransactionFailed,
    #[error("failed to commit search users transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait SearchUsersUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchUsersRequest,
    ) -> Result<SearchUsersResult, SearchUsersError>;
}

pub struct SearchUsersHandler<U, R, A> {
    unit_of_work: U,
    reader: R,
    admin_reader: A,
}

impl<U, R, A> SearchUsersHandler<U, R, A> {
    pub fn new(unit_of_work: U, reader: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            reader,
            admin_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> SearchUsersUseCase for SearchUsersHandler<U, R, A>
where
    U: UnitOfWork,
    R: UserSearchReaderFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "search_users",
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
        request: SearchUsersRequest,
    ) -> Result<SearchUsersResult, SearchUsersError> {
        require_admin_actor_credential(context, CredentialCapability::UsersRead)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SearchUsersError::BeginTransactionFailed)?;
        authorize_search_users(context, &mut tx, &self.admin_reader).await?;
        let result = self.reader.in_transaction(&mut tx).search(&request).await?;
        tx.commit()
            .await
            .map_err(|_| SearchUsersError::CommitTransactionFailed)?;

        Ok(result)
    }
}

async fn authorize_search_users<Tx, A>(
    context: &OperationContext,
    tx: &mut Tx,
    admin_reader: &A,
) -> Result<(), SearchUsersError>
where
    Tx: Transaction,
    A: UserAdminReaderFactory<Tx>,
{
    match &context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => {
            let mut reader = admin_reader.in_transaction(tx);
            require_admin_actor(context, &mut reader)
                .await
                .map_err(SearchUsersError::from)
        }
        Principal::Anonymous => Err(SearchUsersError::AuthenticatedActorRequired),
    }
}

impl From<RequireAdminActorError> for SearchUsersError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for SearchUsersError {
    fn from(error: UserAdminReadError) -> Self {
        match error {
            UserAdminReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAdminReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserAdminReadError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<UserSearchReadError> for SearchUsersError {
    fn from(error: UserSearchReadError) -> Self {
        match error {
            UserSearchReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserSearchReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserSearchReadError::Internal { source } => Self::Internal { source },
        }
    }
}
