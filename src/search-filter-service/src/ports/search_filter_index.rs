use crate::ports::SearchFilterView;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::user_search_filter_id::UserSearchFilterId;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchFilterIndexQuery {
    pub state: Option<common::resource_state::domain::ResourceState>,
    pub has_enhanced_search_description: Option<bool>,
    pub cursor: Option<Cursor<serde_json::Value>>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterIndexError {
    #[error("search filter index write failed")]
    WriteFailed,
    #[error("search filter index delete failed")]
    DeleteFailed,
    #[error("search filter percolation failed")]
    PercolateFailed,
    #[error("search filter query failed")]
    QueryFailed,
    #[error("search filter document invalid")]
    InvalidDocument,
}

#[async_trait::async_trait]
pub trait SearchFilterIndex: Send + Sync {
    async fn index(&self, filter: &SearchFilterView) -> Result<(), SearchFilterIndexError>;
    async fn delete(&self, id: UserSearchFilterId) -> Result<(), SearchFilterIndexError>;
    async fn percolate(
        &self,
        product_document: serde_json::Value,
    ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError>;
    async fn query(
        &self,
        query: &SearchFilterIndexQuery,
    ) -> Result<CursoredResult<SearchFilterView, serde_json::Value>, SearchFilterIndexError>;
}
