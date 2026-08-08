use crate::ports::{SearchFilterProjection, SearchFilterView};
use common::error::boxed::BoxError;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::user_search_filter_id::UserSearchFilterId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterProjectionWriteOutcome {
    Applied,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchFilterIndexQuery {
    pub state: Option<common::resource_state::domain::ResourceState>,
    pub has_enhanced_search_description: Option<bool>,
    pub cursor: Option<Cursor<serde_json::Value>>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterIndexError {
    #[error("search filter index write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter index delete failed")]
    DeleteFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter percolation failed")]
    PercolateFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter document invalid")]
    InvalidDocument {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait SearchFilterIndex: Send + Sync {
    async fn upsert(
        &self,
        projection: &SearchFilterProjection,
    ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError>;
    async fn delete(
        &self,
        id: UserSearchFilterId,
        source_version: i64,
    ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError>;
    async fn percolate(
        &self,
        product_document: serde_json::Value,
    ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError>;
    async fn query(
        &self,
        query: &SearchFilterIndexQuery,
    ) -> Result<CursoredResult<SearchFilterView, serde_json::Value>, SearchFilterIndexError>;
}
