use crate::ports::PersistedSearchFilter;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use search_filter_core::ProductSearch;
use search_filter_core::ResourceState;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterView {
    pub search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    pub state: ResourceState,
    pub search: ProductSearch,
    pub embedding: Option<Vec<f32>>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub last_hybrid_search_matched: OffsetDateTime,
}

impl From<PersistedSearchFilter> for SearchFilterView {
    fn from(persisted: PersistedSearchFilter) -> Self {
        let filter = persisted.filter;
        Self {
            search_filter_id: filter.id(),
            user_id: filter.user_id(),
            name: filter.name().clone(),
            notifications: filter.notifications(),
            state: filter.state(),
            search: filter.search().clone(),
            embedding: filter.embedding().cloned(),
            created: persisted.created,
            updated: persisted.updated,
            last_hybrid_search_matched: persisted.last_hybrid_search_matched,
        }
    }
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

    async fn find_for_user_by_id(
        &self,
        user_id: UserId,
        search_filter_id: UserSearchFilterId,
    ) -> Result<Option<SearchFilterView>, SearchFilterReadError> {
        Ok(self
            .find_for_user(user_id)
            .await?
            .into_iter()
            .find(|view| view.search_filter_id == search_filter_id))
    }
}
