use crate::ports::{
    SearchFilterQuotaReadError, SearchFilterQuotaReader, SearchFilterQuotaReaderFactory,
    SearchFilterRepository, SearchFilterRepositoryError, SearchFilterRepositoryFactory,
    SearchFilterView,
};
use crate::tier_policy::{active_filter_quota, validate_search_features};
use crate::use_cases::embedding_query;
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::{BoxError, box_error};
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::resource_state::domain::ResourceState;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;

use common::user_search_filter_name::UserSearchFilterName;
use embedding::{EmbeddingError, EmbeddingGenerator};
use search_filter_core::{NewSearchFilter, ProductSearch, SearchFilter};
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};

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
    #[error("user tier entitlement lock failed")]
    UserTierEntitlementsLockFailed {
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

pub struct CreateSearchFilterHandler<U, R, E, Q, A> {
    unit_of_work: U,
    filters: R,
    embeddings: E,
    quotas: Q,
    tier_entitlements: A,
}

impl<U, R, E, Q, A> CreateSearchFilterHandler<U, R, E, Q, A> {
    pub fn new(
        unit_of_work: U,
        filters: R,
        embeddings: E,
        quotas: Q,
        tier_entitlements: A,
    ) -> Self {
        Self {
            unit_of_work,
            filters,
            embeddings,
            quotas,
            tier_entitlements,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, E, Q, A> CreateSearchFilterUseCase for CreateSearchFilterHandler<U, R, E, Q, A>
where
    U: UnitOfWork,
    R: SearchFilterRepositoryFactory<U::Tx>,
    E: EmbeddingGenerator,
    Q: SearchFilterQuotaReaderFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
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
        let embedding = match embedding_query(&command.search).map_err(embedding_error)? {
            Some(query) => Some(
                self.embeddings
                    .embed_search_query(&query)
                    .await
                    .map_err(embedding_error)?
                    .into_values(),
            ),
            None => None,
        };
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
        let tier = self
            .tier_entitlements
            .in_transaction(&mut tx)
            .lock_user_tier(command.user_id)
            .await
            .map_err(tier_entitlements_error)?
            .ok_or(CreateSearchFilterError::UserNotFound)?;
        validate_search_features(tier, filter.search()).map_err(|feature| {
            CreateSearchFilterError::SearchFilterFeatureRestricted { feature }
        })?;
        let quota = active_filter_quota(tier);
        let active_count = self
            .quotas
            .in_transaction(&mut tx)
            .count_active_for_user(command.user_id)
            .await
            .map_err(search_filter_quota_read_error)?;
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

fn embedding_error(error: EmbeddingError) -> CreateSearchFilterError {
    CreateSearchFilterError::EmbeddingGenerationFailed {
        source: box_error(error),
    }
}

fn tier_entitlements_error(error: UserTierEntitlementsError) -> CreateSearchFilterError {
    match error {
        UserTierEntitlementsError::LockFailed { source }
        | UserTierEntitlementsError::ReconciliationFailed { source } => {
            CreateSearchFilterError::UserTierEntitlementsLockFailed { source }
        }
    }
}

fn search_filter_quota_read_error(error: SearchFilterQuotaReadError) -> CreateSearchFilterError {
    CreateSearchFilterError::SearchFilterQuotaReadFailed {
        source: box_error(error),
    }
}

fn repository_error(error: SearchFilterRepositoryError) -> CreateSearchFilterError {
    match error {
        SearchFilterRepositoryError::AlreadyExists => {
            CreateSearchFilterError::SearchFilterAlreadyExists
        }
        error @ SearchFilterRepositoryError::InvalidPersistedState { .. } => {
            CreateSearchFilterError::PersistedSearchFilterStateInvalid { source: error }
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
