use crate::create_receipt::CreateSearchFilterIntent;
use crate::ports::{
    SearchFilterQuotaReadError, SearchFilterQuotaReader, SearchFilterQuotaReaderFactory,
    SearchFilterRepository, SearchFilterRepositoryError, SearchFilterRepositoryFactory,
    SearchFilterView,
};
use crate::tier_policy::{active_filter_quota, matching_evidence_policy, validate_search_features};
use crate::use_cases::embedding_query;
use application::error::{BoxError, box_error};
use application::idempotency::{
    CommandReceiptError, CommandReceipts, CommandReceiptsFactory, IdempotencyKey,
};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use user_core::user_id::UserId;

use embedding::{EmbeddingError, EmbeddingGenerator};
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use search_filter_core::{NewSearchFilter, SearchFilter};
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateSearchFilterCommand {
    pub idempotency_key: Option<IdempotencyKey>,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    pub search: ProductListingSearch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateSearchFilterResult {
    pub filter: SearchFilterView,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateSearchFilterError {
    #[error(transparent)]
    Receipt(#[from] CommandReceiptError),
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
    BeginTransactionFailed(#[source] application::error::BoxError),
    #[error("failed to commit search filter transaction")]
    CommitTransactionFailed(#[source] application::error::BoxError),
}

#[async_trait::async_trait]
pub trait CreateSearchFilterUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateSearchFilterCommand,
    ) -> Result<CreateSearchFilterResult, CreateSearchFilterError>;
}

pub struct CreateSearchFilterHandler<U, R, E, Q, A, C> {
    unit_of_work: U,
    filters: R,
    embeddings: E,
    quotas: Q,
    tier_entitlements: A,
    receipts: C,
}

impl<U, R, E, Q, A, C> CreateSearchFilterHandler<U, R, E, Q, A, C> {
    pub fn new(
        unit_of_work: U,
        filters: R,
        embeddings: E,
        quotas: Q,
        tier_entitlements: A,
        receipts: C,
    ) -> Self {
        Self {
            unit_of_work,
            filters,
            embeddings,
            quotas,
            tier_entitlements,
            receipts,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, E, Q, A, C> CreateSearchFilterUseCase for CreateSearchFilterHandler<U, R, E, Q, A, C>
where
    U: UnitOfWork,
    R: SearchFilterRepositoryFactory<U::Tx>,
    E: EmbeddingGenerator,
    Q: SearchFilterQuotaReaderFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
    C: CommandReceiptsFactory<U::Tx, CreateSearchFilterIntent, CreateSearchFilterResult>,
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
        let intent = command
            .idempotency_key
            .clone()
            .map(|key| CreateSearchFilterIntent::new(context, &command, key));
        // A completed retry must not pay for provider preparation or fail on its own quota use.
        if let Some(intent) = &intent {
            let mut tx = self.unit_of_work.begin().await.map_err(|source| {
                CreateSearchFilterError::BeginTransactionFailed(Box::new(source))
            })?;
            self.receipts
                .in_transaction(&mut tx)
                .coordinate(intent)
                .await
                .map_err(receipt_error)?;
            self.tier_entitlements
                .in_transaction(&mut tx)
                .lock_user_tier(command.user_id)
                .await
                .map_err(tier_entitlements_error)?
                .ok_or(CreateSearchFilterError::UserNotFound)?;
            let completed = self
                .receipts
                .in_transaction(&mut tx)
                .find(intent)
                .await
                .map_err(receipt_error)?;
            if let Some(result) = completed {
                tx.commit().await.map_err(|source| {
                    CreateSearchFilterError::CommitTransactionFailed(Box::new(source))
                })?;
                tracing::info!(
                    event = "command.replayed",
                    operation = "CREATE_SEARCH_FILTER",
                    outcome = "success"
                );
                return Ok(result);
            }
            // Explicitly finish this read transaction: SQLx rollback on drop is asynchronous,
            // so merely dropping it can retain locks into provider I/O or our next attempt.
            tx.commit().await.map_err(|source| {
                CreateSearchFilterError::CommitTransactionFailed(Box::new(source))
            })?;
        }
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
        let mut filter = SearchFilter::create(NewSearchFilter {
            user_search_filter_id: UserSearchFilterId::new(),
            user_id: command.user_id,
            name: command.name,
            notifications: command.notifications,
            state: SearchFilterState::Active,
            search: command.search,
            embedding,
            matching_evidence_policy: search_filter_core::MatchingEvidencePolicy::TextOnly,
        });

        let mut tx =
            self.unit_of_work.begin().await.map_err(|source| {
                CreateSearchFilterError::BeginTransactionFailed(Box::new(source))
            })?;
        if let Some(intent) = &intent {
            self.receipts
                .in_transaction(&mut tx)
                .coordinate(intent)
                .await
                .map_err(receipt_error)?;
        }
        let tier = self
            .tier_entitlements
            .in_transaction(&mut tx)
            .lock_user_tier(command.user_id)
            .await
            .map_err(tier_entitlements_error)?
            .ok_or(CreateSearchFilterError::UserNotFound)?;
        if let Some(intent) = &intent {
            let completed = self
                .receipts
                .in_transaction(&mut tx)
                .find(intent)
                .await
                .map_err(receipt_error)?;
            if let Some(result) = completed {
                tx.commit().await.map_err(|source| {
                    CreateSearchFilterError::CommitTransactionFailed(Box::new(source))
                })?;
                tracing::info!(
                    event = "command.replayed",
                    operation = "CREATE_SEARCH_FILTER",
                    outcome = "success"
                );
                return Ok(result);
            }
        }
        validate_search_features(tier, filter.search()).map_err(|feature| {
            CreateSearchFilterError::SearchFilterFeatureRestricted { feature }
        })?;
        filter.resolve_matching_evidence(matching_evidence_policy(tier));
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
        let result = CreateSearchFilterResult {
            filter: persisted.into(),
        };
        if let Some(intent) = &intent {
            self.receipts
                .in_transaction(&mut tx)
                .complete(intent, &result)
                .await
                .map_err(receipt_error)?;
        }
        tx.commit()
            .await
            .map_err(|source| CreateSearchFilterError::CommitTransactionFailed(Box::new(source)))?;
        tracing::info!(
            event = "search_filter.created",
            actor_type = context.principal.kind(),
            actor_id = ?context.principal.actor_id(),
            search_filter_id = %result.filter.search_filter_id,
            outcome = "success",
        );

        Ok(result)
    }
}

fn receipt_error(error: CommandReceiptError) -> CreateSearchFilterError {
    let outcome = match &error {
        CommandReceiptError::Mismatch => "mismatch",
        CommandReceiptError::InProgress => "in_progress",
        CommandReceiptError::ResultUnavailable => "result_unavailable",
        CommandReceiptError::Storage(_) => "storage_failed",
        CommandReceiptError::InvalidPersistedState(_) => "invalid_receipt",
    };
    tracing::info!(
        event = "command.retry",
        operation = "CREATE_SEARCH_FILTER",
        outcome
    );
    error.into()
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
