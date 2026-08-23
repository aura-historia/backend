use crate::ports::SearchFilterView;
use application::error::BoxError;
use search_filter_core::user_search_filter_id::UserSearchFilterId;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterProjection {
    pub view: SearchFilterView,
    pub source_version: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterIndexReadError {
    #[error("search filter projection read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted search filter projection state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait SearchFilterIndexReader: Send + Sync {
    async fn find_by_id(
        &self,
        search_filter_id: UserSearchFilterId,
    ) -> Result<Option<SearchFilterProjection>, SearchFilterIndexReadError>;

    async fn list_after(
        &self,
        after: Option<UserSearchFilterId>,
        limit: usize,
    ) -> Result<Vec<SearchFilterProjection>, SearchFilterIndexReadError>;
}
