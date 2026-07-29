use common::user_id::UserId;
use search_filter_core::SearchFilter;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterView {
    pub filter: SearchFilter,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub last_hybrid_search_matched: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterReadError {
    #[error("search filter read failed")]
    ReadFailed,
    #[error("persisted search filter state is invalid")]
    InvalidPersistedState,
}

#[async_trait::async_trait]
pub trait SearchFilterReader: Send + Sync {
    async fn find_for_user(
        &self,
        user_id: UserId,
    ) -> Result<Vec<SearchFilterView>, SearchFilterReadError>;
}
