use crate::ports::{
    SearchFilterEmbeddingGenerationError, SearchFilterEmbeddingGenerator, SearchFilterReadError,
    SearchFilterReader, SearchFilterRepository, SearchFilterRepositoryError,
    SearchFilterRepositoryFactory, SearchFilterView,
};
use crate::tier_policy::{active_filter_quota, validate_search_features};
use common::error::boxed::{BoxError, box_error};
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::resource_state::domain::ResourceState;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;

use common::user_search_filter_name::UserSearchFilterName;
use search_filter_core::{NewSearchFilter, ProductSearch, SearchFilter};
use user_service::ports::{UserAccountReadError, UserAccountReader, UserAccountReaderFactory};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateSearchFilterCommand {
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    pub search: ProductSearch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateSearchFilterResult {
    pub filter: SearchFilterView,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateSearchFilterError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("actor may not manage this search filter")]
    ActorMayNotManageSearchFilter,
    #[error("user not found")]
    UserNotFound,
    #[error(
        "search filter quota exceeded: {active_count}/{quota} active filters are already in use"
    )]
    SearchFilterQuotaExceeded { active_count: usize, quota: usize },
    #[error("search filter feature '{feature}' requires a higher user tier")]
    SearchFilterFeatureRestricted { feature: &'static str },
    #[error("user account read failed")]
    UserAccountReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter quota read failed")]
    SearchFilterQuotaReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter already exists")]
    SearchFilterAlreadyExists,
    #[error("search filter embedding generation failed")]
    EmbeddingGenerationFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter insert failed")]
    SearchFilterInsertFailed {
        #[source]
        source: SearchFilterRepositoryError,
    },
    #[error("persisted search filter state is invalid")]
    PersistedSearchFilterStateInvalid {
        #[source]
        source: SearchFilterRepositoryError,
    },
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

pub struct CreateSearchFilterHandler<U, R, E, F, A> {
    unit_of_work: U,
    filters: R,
    embeddings: E,
    filter_reader: F,
    accounts: A,
}

impl<U, R, E, F, A> CreateSearchFilterHandler<U, R, E, F, A> {
    pub fn new(unit_of_work: U, filters: R, embeddings: E, filter_reader: F, accounts: A) -> Self {
        Self {
            unit_of_work,
            filters,
            embeddings,
            filter_reader,
            accounts,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, E, F, A> CreateSearchFilterUseCase for CreateSearchFilterHandler<U, R, E, F, A>
where
    U: UnitOfWork,
    R: SearchFilterRepositoryFactory<U::Tx>,
    E: SearchFilterEmbeddingGenerator,
    F: SearchFilterReader,
    A: UserAccountReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "create_search_filter",
        skip_all,
        fields(
            user_id = %command.user_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateSearchFilterCommand,
    ) -> Result<CreateSearchFilterResult, CreateSearchFilterError> {
        authorize_owner(context, command.user_id)?;
        let embedding = self
            .embeddings
            .generate(&command.search)
            .await
            .map_err(embedding_error)?;
        let filter = SearchFilter::create(NewSearchFilter {
            user_search_filter_id: UserSearchFilterId::new(),
            user_id: command.user_id,
            name: command.name,
            notifications: command.notifications,
            state: ResourceState::Active,
            search: command.search,
            embedding,
        });

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateSearchFilterError::BeginTransactionFailed)?;
        let account = self
            .accounts
            .in_transaction(&mut tx)
            .find_by_id(command.user_id)
            .await
            .map_err(user_account_read_error)?
            .ok_or(CreateSearchFilterError::UserNotFound)?;
        validate_search_features(account.tier, filter.search()).map_err(|feature| {
            CreateSearchFilterError::SearchFilterFeatureRestricted { feature }
        })?;
        let quota = active_filter_quota(account.tier);
        let active_count = self
            .filter_reader
            .find_for_user(command.user_id)
            .await
            .map_err(search_filter_quota_read_error)?
            .into_iter()
            .filter(|filter| filter.state.is_active())
            .count();
        if active_count >= quota {
            return Err(CreateSearchFilterError::SearchFilterQuotaExceeded {
                active_count,
                quota,
            });
        }
        let persisted = self
            .filters
            .in_transaction(&mut tx)
            .insert(&filter)
            .await
            .map_err(repository_error)?;
        tx.commit()
            .await
            .map_err(|_| CreateSearchFilterError::CommitTransactionFailed)?;
        tracing::info!(
            event = "search_filter.created",
            actor_type = context.principal.kind(),
            actor_id = ?context.principal.actor_id(),
            search_filter_id = %persisted.filter.id(),
            outcome = "success",
        );

        Ok(CreateSearchFilterResult {
            filter: persisted.into(),
        })
    }
}

fn authorize_owner(
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

fn embedding_error(error: SearchFilterEmbeddingGenerationError) -> CreateSearchFilterError {
    CreateSearchFilterError::EmbeddingGenerationFailed {
        source: box_error(error),
    }
}

fn user_account_read_error(error: UserAccountReadError) -> CreateSearchFilterError {
    CreateSearchFilterError::UserAccountReadFailed {
        source: box_error(error),
    }
}

fn search_filter_quota_read_error(error: SearchFilterReadError) -> CreateSearchFilterError {
    CreateSearchFilterError::SearchFilterQuotaReadFailed {
        source: box_error(error),
    }
}

fn repository_error(error: SearchFilterRepositoryError) -> CreateSearchFilterError {
    match error {
        SearchFilterRepositoryError::AlreadyExists => {
            CreateSearchFilterError::SearchFilterAlreadyExists
        }
        SearchFilterRepositoryError::InvalidPersistedState => {
            CreateSearchFilterError::PersistedSearchFilterStateInvalid {
                source: SearchFilterRepositoryError::InvalidPersistedState,
            }
        }
        error => CreateSearchFilterError::SearchFilterInsertFailed { source: error },
    }
}

impl From<OperationAuthorizationError> for CreateSearchFilterError {
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
