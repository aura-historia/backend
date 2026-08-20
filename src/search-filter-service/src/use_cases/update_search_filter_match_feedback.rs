use crate::ports::{
    SearchFilterMatchRepository, SearchFilterMatchRepositoryError,
    SearchFilterMatchRepositoryFactory, SearchFilterMatchView, SearchFilterRepository,
    SearchFilterRepositoryError, SearchFilterRepositoryFactory,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::{BoxError, box_error};
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::patch_field::PatchField;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use product_core::product_id::ProductId;

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateSearchFilterMatchFeedbackCommand {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub product_id: ProductId,
    pub feedback: PatchField<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateSearchFilterMatchFeedbackResult {
    pub search_filter_match: SearchFilterMatchView,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateSearchFilterMatchFeedbackError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("actor may not manage this search filter")]
    ActorMayNotManageSearchFilter,
    #[error("search filter not found")]
    SearchFilterNotFound,
    #[error("search filter match not found")]
    SearchFilterMatchNotFound,

    #[error("search filter lookup failed")]
    SearchFilterLookupFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter match lookup failed")]
    SearchFilterMatchLookupFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter match update failed")]
    SearchFilterMatchUpdateFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted search filter state is invalid")]
    PersistedSearchFilterStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("persisted search filter match state is invalid")]
    PersistedSearchFilterMatchStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search filter transaction")]
    BeginTransactionFailed,
    #[error("failed to commit search filter transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateSearchFilterMatchFeedbackUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateSearchFilterMatchFeedbackCommand,
    ) -> Result<UpdateSearchFilterMatchFeedbackResult, UpdateSearchFilterMatchFeedbackError>;
}

pub struct UpdateSearchFilterMatchFeedbackHandler<U, F, M> {
    unit_of_work: U,
    filters: F,
    matches: M,
}

impl<U, F, M> UpdateSearchFilterMatchFeedbackHandler<U, F, M> {
    pub fn new(unit_of_work: U, filters: F, matches: M) -> Self {
        Self {
            unit_of_work,
            filters,
            matches,
        }
    }
}

#[async_trait::async_trait]
impl<U, F, M> UpdateSearchFilterMatchFeedbackUseCase
    for UpdateSearchFilterMatchFeedbackHandler<U, F, M>
where
    U: UnitOfWork,
    F: SearchFilterRepositoryFactory<U::Tx>,
    M: SearchFilterMatchRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "update_search_filter_match_feedback",
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
        command: UpdateSearchFilterMatchFeedbackCommand,
    ) -> Result<UpdateSearchFilterMatchFeedbackResult, UpdateSearchFilterMatchFeedbackError> {
        authorize_owner(context, command.user_id)?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateSearchFilterMatchFeedbackError::BeginTransactionFailed)?;
        let filter = self
            .filters
            .in_transaction(&mut tx)
            .find_by_id(command.search_filter_id)
            .await
            .map_err(search_filter_repository_error)?
            .filter(|persisted| persisted.filter.user_id() == command.user_id)
            .ok_or(UpdateSearchFilterMatchFeedbackError::SearchFilterNotFound)?;
        let mut persisted_search_filter_match = self
            .matches
            .in_transaction(&mut tx)
            .find_by_filter_and_product(filter.filter.id(), command.product_id)
            .await
            .map_err(search_filter_match_lookup_error)?
            .filter(|persisted| persisted.product_match.user_id == command.user_id)
            .ok_or(UpdateSearchFilterMatchFeedbackError::SearchFilterMatchNotFound)?;

        let mut search_filter_match = persisted_search_filter_match.product_match.clone();
        let feedback = match command.feedback {
            PatchField::Unchanged => None,
            PatchField::Set(value) => Some(Some(value)),
            PatchField::Clear => Some(None),
        };
        if let Some(feedback) = feedback
            && search_filter_match.change_feedback(feedback).changed()
        {
            persisted_search_filter_match = self
                .matches
                .in_transaction(&mut tx)
                .update(&search_filter_match)
                .await
                .map_err(search_filter_match_update_error)?;
        }

        tx.commit()
            .await
            .map_err(|_| UpdateSearchFilterMatchFeedbackError::CommitTransactionFailed)?;
        tracing::info!(
            event = "search_filter_match.feedback_updated",
            actor_type = context.principal.kind(),
            actor_id = ?context.principal.actor_id(),
            search_filter_id = %persisted_search_filter_match.product_match.user_search_filter_id,
            outcome = "success",
        );
        Ok(UpdateSearchFilterMatchFeedbackResult {
            search_filter_match: persisted_search_filter_match.into(),
        })
    }
}

fn authorize_owner(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), UpdateSearchFilterMatchFeedbackError> {
    context
        .require()
        .credential_capability(CredentialCapability::SearchFiltersWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<UpdateSearchFilterMatchFeedbackError>()
}

fn search_filter_repository_error(
    error: SearchFilterRepositoryError,
) -> UpdateSearchFilterMatchFeedbackError {
    match error {
        SearchFilterRepositoryError::InvalidPersistedState { source } => {
            UpdateSearchFilterMatchFeedbackError::PersistedSearchFilterStateInvalid { source }
        }
        error => UpdateSearchFilterMatchFeedbackError::SearchFilterLookupFailed {
            source: box_error(error),
        },
    }
}

fn search_filter_match_lookup_error(
    error: SearchFilterMatchRepositoryError,
) -> UpdateSearchFilterMatchFeedbackError {
    match error {
        SearchFilterMatchRepositoryError::InvalidPersistedState => {
            UpdateSearchFilterMatchFeedbackError::PersistedSearchFilterMatchStateInvalid {
                source: box_error(SearchFilterMatchRepositoryError::InvalidPersistedState),
            }
        }
        error => UpdateSearchFilterMatchFeedbackError::SearchFilterMatchLookupFailed {
            source: box_error(error),
        },
    }
}

fn search_filter_match_update_error(
    error: SearchFilterMatchRepositoryError,
) -> UpdateSearchFilterMatchFeedbackError {
    match error {
        SearchFilterMatchRepositoryError::InvalidPersistedState => {
            UpdateSearchFilterMatchFeedbackError::PersistedSearchFilterMatchStateInvalid {
                source: box_error(SearchFilterMatchRepositoryError::InvalidPersistedState),
            }
        }
        error => UpdateSearchFilterMatchFeedbackError::SearchFilterMatchUpdateFailed {
            source: box_error(error),
        },
    }
}

impl From<OperationAuthorizationError> for UpdateSearchFilterMatchFeedbackError {
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
