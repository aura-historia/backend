use common::enhanced_match_reason::EnhancedMatchReason;
use common::event_id::EventId;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::product_id::ProductId;
use common::sort::SortOrder;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterMatchView {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub search_filter_name: Option<UserSearchFilterName>,
    pub product_id: ProductId,
    pub origin_event_id: EventId,
    pub enhanced_match_reason: Option<EnhancedMatchReason>,
    pub feedback: Option<bool>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterMatchListQuery {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub cursor: Option<Cursor<OffsetDateTime>>,
    pub order: SortOrder,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterMatchReadError {
    #[error("search filter match read failed")]
    ReadFailed,
    #[error("persisted search filter match state is invalid")]
    InvalidPersistedState,
}

#[async_trait::async_trait]
pub trait SearchFilterMatchReader: Send + Sync {
    async fn list_for_owned_filter(
        &self,
        query: &SearchFilterMatchListQuery,
    ) -> Result<
        Option<CursoredResult<SearchFilterMatchView, OffsetDateTime>>,
        SearchFilterMatchReadError,
    >;
}
