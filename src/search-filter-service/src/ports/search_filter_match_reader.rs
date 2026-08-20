use crate::ports::PersistedSearchFilterMatch;
use application::pagination::{Cursor, CursoredResult};
use domain_primitives::event_id::EventId;
use domain_primitives::sort::SortOrder;
use product_core::product_id::ProductId;
use search_filter_core::enhanced_match_reason::EnhancedMatchReason;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use time::OffsetDateTime;
use user_core::user_id::UserId;

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

impl From<PersistedSearchFilterMatch> for SearchFilterMatchView {
    fn from(persisted: PersistedSearchFilterMatch) -> Self {
        let product_match = persisted.product_match;
        Self {
            user_id: product_match.user_id,
            search_filter_id: product_match.user_search_filter_id,
            search_filter_name: product_match.user_search_filter_name,
            product_id: product_match.product_id,
            origin_event_id: product_match.origin_event_id,
            enhanced_match_reason: product_match.enhanced_match_reason,
            feedback: product_match.feedback,
            created: persisted.created,
            updated: persisted.updated,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchFilterMatchCursor {
    pub created: OffsetDateTime,
    pub product_id: ProductId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFilterMatchListItem {
    pub product_id: ProductId,
    pub created: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterMatchListQuery {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub cursor: Option<Cursor<SearchFilterMatchCursor>>,
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
        Option<CursoredResult<SearchFilterMatchListItem, SearchFilterMatchCursor>>,
        SearchFilterMatchReadError,
    >;
}
