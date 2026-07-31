use crate::ports::{
    SearchFilterRepository, SearchFilterRepositoryError, SearchFilterRepositoryFactory,
};
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::resource_state::domain::ResourceState;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use product_core::product_search::ProductSearch;
use search_filter_core::{NewSearchFilter, SearchFilter};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateSearchFilterCommand {
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    pub search: ProductSearch,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateSearchFilterResult {
    pub filter: SearchFilter,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateSearchFilterError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("search filter already exists")]
    AlreadyExists,
    #[error("temporary search filter persistence failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted search filter state")]
    InvalidPersistedState,
    #[error("failed to begin search filter transaction")]
    BeginTransactionFailed,
    #[error("failed to commit search filter transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CreateSearchFilterUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateSearchFilterCommand,
    ) -> Result<CreateSearchFilterResult, CreateSearchFilterError>;
}

pub struct CreateSearchFilterHandler<U, R> {
    unit_of_work: U,
    filters: R,
}

impl<U, R> CreateSearchFilterHandler<U, R> {
    pub fn new(unit_of_work: U, filters: R) -> Self {
        Self {
            unit_of_work,
            filters,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> CreateSearchFilterUseCase for CreateSearchFilterHandler<U, R>
where
    U: UnitOfWork,
    R: SearchFilterRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(name = "create_search_filter", skip_all, fields(user_id = %command.user_id, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateSearchFilterCommand,
    ) -> Result<CreateSearchFilterResult, CreateSearchFilterError> {
        authorize_create(context, command.user_id)?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateSearchFilterError::BeginTransactionFailed)?;
        let filter = SearchFilter::create(NewSearchFilter {
            user_search_filter_id: UserSearchFilterId::new(),
            user_id: command.user_id,
            name: command.name,
            notifications: command.notifications,
            state: ResourceState::Active,
            search: command.search,
            embedding: command.embedding,
        });
        self.filters.in_transaction(&mut tx).insert(&filter).await?;
        tx.commit()
            .await
            .map_err(|_| CreateSearchFilterError::CommitTransactionFailed)?;
        Ok(CreateSearchFilterResult { filter })
    }
}

fn authorize_create(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), CreateSearchFilterError> {
    context
        .require()
        .credential_capability(CredentialCapability::SearchFiltersWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<CreateSearchFilterError>()
}

impl From<OperationAuthorizationError> for CreateSearchFilterError {
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

impl From<SearchFilterRepositoryError> for CreateSearchFilterError {
    fn from(value: SearchFilterRepositoryError) -> Self {
        match value {
            SearchFilterRepositoryError::AlreadyExists => CreateSearchFilterError::AlreadyExists,
            SearchFilterRepositoryError::InvalidPersistedState => {
                CreateSearchFilterError::InvalidPersistedState
            }
            SearchFilterRepositoryError::LookupFailed
            | SearchFilterRepositoryError::InsertFailed
            | SearchFilterRepositoryError::UpdateFailed
            | SearchFilterRepositoryError::DeleteFailed => {
                CreateSearchFilterError::TemporarilyUnavailable
            }
        }
    }
}
