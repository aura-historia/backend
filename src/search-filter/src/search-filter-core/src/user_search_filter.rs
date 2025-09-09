use crate::{search_filter::SearchFilter, search_filter_id::SearchFilterId};
use common::user_id::UserId;
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct UserSearchFilter {
    pub user_id: UserId,
    pub search_filter_id: SearchFilterId,
    pub search_filter: SearchFilter,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}
