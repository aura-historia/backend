use crate::ports::PersistedSearchFilter;
use product_core::product_search::ProductSearch;
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterView {
    pub search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    pub state: SearchFilterState,
    pub search: ProductSearch,
    pub embedding: Option<Vec<f32>>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
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
