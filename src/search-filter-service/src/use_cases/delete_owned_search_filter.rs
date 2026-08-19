use crate::ports::{
    SearchFilterRepository, SearchFilterRepositoryError, SearchFilterRepositoryFactory,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::{BoxError, box_error};
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteOwnedSearchFilterCommand {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteOwnedSearchFilterResult;

#[derive(Debug, thiserror::Error)]
pub enum DeleteOwnedSearchFilterError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("actor may not manage this search filter")]
    ActorMayNotManageSearchFilter,
    #[error("search filter not found")]
    SearchFilterNotFound,
    #[error("search filter lookup failed")]
    SearchFilterLookupFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter deletion failed")]
    SearchFilterDeletionFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted search filter state is invalid")]
    PersistedSearchFilterStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search filter transaction")]
    BeginTransactionFailed,
    #[error("failed to commit search filter transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait DeleteOwnedSearchFilterUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteOwnedSearchFilterCommand,
    ) -> Result<DeleteOwnedSearchFilterResult, DeleteOwnedSearchFilterError>;
}

pub struct DeleteOwnedSearchFilterHandler<U, R> {
    unit_of_work: U,
    filters: R,
}

impl<U, R> DeleteOwnedSearchFilterHandler<U, R> {
    pub fn new(unit_of_work: U, filters: R) -> Self {
        Self {
            unit_of_work,
            filters,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> DeleteOwnedSearchFilterUseCase for DeleteOwnedSearchFilterHandler<U, R>
where
    U: UnitOfWork,
    R: SearchFilterRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "delete_owned_search_filter",
        skip_all,
        fields(
            search_filter_id = %command.search_filter_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteOwnedSearchFilterCommand,
    ) -> Result<DeleteOwnedSearchFilterResult, DeleteOwnedSearchFilterError> {
        authorize_owner(context, command.user_id)?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| DeleteOwnedSearchFilterError::BeginTransactionFailed)?;
        let filter = self
            .filters
            .in_transaction(&mut tx)
            .find_by_id(command.search_filter_id)
            .await
            .map_err(lookup_error)?
            .filter(|persisted| persisted.filter.user_id() == command.user_id)
            .ok_or(DeleteOwnedSearchFilterError::SearchFilterNotFound)?;
        self.filters
            .in_transaction(&mut tx)
            .delete(filter.filter.id())
            .await
            .map_err(delete_error)?;
        tx.commit()
            .await
            .map_err(|_| DeleteOwnedSearchFilterError::CommitTransactionFailed)?;
        tracing::info!(
            event = "search_filter.deleted",
            actor_type = context.principal.kind(),
            actor_id = ?context.principal.actor_id(),
            search_filter_id = %filter.filter.id(),
            outcome = "success",
        );
        Ok(DeleteOwnedSearchFilterResult)
    }
}

fn authorize_owner(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), DeleteOwnedSearchFilterError> {
    context
        .require()
        .credential_capability(CredentialCapability::SearchFiltersWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<DeleteOwnedSearchFilterError>()
}

fn lookup_error(error: SearchFilterRepositoryError) -> DeleteOwnedSearchFilterError {
    match error {
        SearchFilterRepositoryError::InvalidPersistedState { source } => {
            DeleteOwnedSearchFilterError::PersistedSearchFilterStateInvalid { source }
        }
        error => DeleteOwnedSearchFilterError::SearchFilterLookupFailed {
            source: box_error(error),
        },
    }
}

fn delete_error(error: SearchFilterRepositoryError) -> DeleteOwnedSearchFilterError {
    match error {
        SearchFilterRepositoryError::InvalidPersistedState { source } => {
            DeleteOwnedSearchFilterError::PersistedSearchFilterStateInvalid { source }
        }
        error => DeleteOwnedSearchFilterError::SearchFilterDeletionFailed {
            source: box_error(error),
        },
    }
}

impl From<OperationAuthorizationError> for DeleteOwnedSearchFilterError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => {
                Self::ActorMayNotManageSearchFilter
            }
        }
    }
}
