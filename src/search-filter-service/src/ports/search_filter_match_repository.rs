use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::{
    SearchFilterProductListingMatch, user_search_filter_id::UserSearchFilterId,
};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct PersistedSearchFilterMatch {
    pub product_match: SearchFilterProductListingMatch,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
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
        product_listing_id: ProductListingId,
    ) -> Result<Option<PersistedSearchFilterMatch>, SearchFilterMatchRepositoryError>;
    async fn insert(
        &mut self,
        product_match: &SearchFilterProductListingMatch,
    ) -> Result<PersistedSearchFilterMatch, SearchFilterMatchRepositoryError>;
    async fn update(
        &mut self,
        product_match: &SearchFilterProductListingMatch,
    ) -> Result<PersistedSearchFilterMatch, SearchFilterMatchRepositoryError>;
}

pub trait SearchFilterMatchRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl SearchFilterMatchRepository + 'tx;
}
