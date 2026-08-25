use application::error::BoxError;
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::{
    search_filter_state::SearchFilterState, user_search_filter_id::UserSearchFilterId,
    user_search_filter_name::UserSearchFilterName,
};
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodicSearchFilterCandidatePageRequest {
    pub after: Option<UserSearchFilterId>,
    pub page_size: usize,
    pub eligible_at_or_before: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicSearchFilterCandidate {
    pub search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub version: i64,
    pub state: SearchFilterState,
    pub search: ProductListingSearch,
    pub embedding: Option<Vec<f32>>,
    pub created: OffsetDateTime,
    pub matched_through: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum PeriodicSearchFilterCandidateReadError {
    #[error("periodic search-filter candidate read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("periodic search-filter candidate state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PeriodicSearchFilterCandidateReader: Send + Sync {
    async fn find_active_page(
        &self,
        request: PeriodicSearchFilterCandidatePageRequest,
    ) -> Result<Vec<PeriodicSearchFilterCandidate>, PeriodicSearchFilterCandidateReadError>;
}
