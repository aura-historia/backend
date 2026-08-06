use common::product_id::ProductId;
use common::user_search_filter_id::UserSearchFilterId;
use search_filter_core::{SearchFilter, SearchFilterProductMatch};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct PersistedSearchFilter {
    pub filter: SearchFilter,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub last_hybrid_search_matched: OffsetDateTime,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersistedSearchFilterMatch {
    pub product_match: SearchFilterProductMatch,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterRepositoryError {
    #[error("search filter already exists")]
    AlreadyExists,
    #[error("search filter lookup failed")]
    LookupFailed,
    #[error("search filter insert failed")]
    InsertFailed,
    #[error("search filter update failed")]
    UpdateFailed,
    #[error("search filter update conflicted with a concurrent write")]
    ConcurrencyConflict,
    #[error("search filter delete failed")]
    DeleteFailed,
    #[error("persisted search filter state is invalid")]
    InvalidPersistedState,
}

#[async_trait::async_trait]
pub trait SearchFilterRepository: Send {
    async fn find_by_id(
        &mut self,
        id: UserSearchFilterId,
    ) -> Result<Option<PersistedSearchFilter>, SearchFilterRepositoryError>;
    async fn insert(
        &mut self,
        filter: &SearchFilter,
    ) -> Result<PersistedSearchFilter, SearchFilterRepositoryError>;
    async fn update(
        &mut self,
        filter: &SearchFilter,
        expected_version: i64,
    ) -> Result<PersistedSearchFilter, SearchFilterRepositoryError>;
    async fn delete(&mut self, id: UserSearchFilterId) -> Result<(), SearchFilterRepositoryError>;
}

pub trait SearchFilterRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl SearchFilterRepository + 'tx;
}

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterMatchRepositoryError {
    #[error("search filter match lookup failed")]
    LookupFailed,
    #[error("search filter match insert failed")]
    InsertFailed,
    #[error("search filter match update failed")]
    UpdateFailed,
    #[error("persisted search filter match is invalid")]
    InvalidPersistedState,
}

#[async_trait::async_trait]
pub trait SearchFilterMatchRepository: Send {
    async fn find_by_filter_and_product(
        &mut self,
        filter_id: UserSearchFilterId,
        product_id: ProductId,
    ) -> Result<Option<PersistedSearchFilterMatch>, SearchFilterMatchRepositoryError>;
    async fn insert(
        &mut self,
        product_match: &SearchFilterProductMatch,
    ) -> Result<PersistedSearchFilterMatch, SearchFilterMatchRepositoryError>;
    async fn update(
        &mut self,
        product_match: &SearchFilterProductMatch,
    ) -> Result<PersistedSearchFilterMatch, SearchFilterMatchRepositoryError>;
}

pub trait SearchFilterMatchRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl SearchFilterMatchRepository + 'tx;
}
