use common::enhanced_match_reason::EnhancedMatchReason;
use common::error::boxed::BoxError;
use common::product_id::ProductId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterMatchCandidate {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub enhanced_match_reason: Option<EnhancedMatchReason>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedSearchFilterMatchCandidate {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub search_filter_name: UserSearchFilterName,
    pub enhanced_match_reason: Option<EnhancedMatchReason>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterMatchCandidateValidationError {
    #[error("search filter match candidate validation failed")]
    ValidationFailed {
        #[source]
        source: BoxError,
    },
    #[error("authoritative search filter state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait SearchFilterMatchCandidateValidator: Send {
    async fn validate_for_product(
        &mut self,
        product_id: ProductId,
        candidates: &[SearchFilterMatchCandidate],
    ) -> Result<Vec<ValidatedSearchFilterMatchCandidate>, SearchFilterMatchCandidateValidationError>;
}

pub trait SearchFilterMatchCandidateValidatorFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl SearchFilterMatchCandidateValidator + 'tx;
}
