use common::{item_state::domain::ItemState, price::domain::MonetaryAmount, user_id::UserId};
use item_dynamodb::item_state_record::ItemStateRecord;
use search_filter_core::{search_filter::SearchFilter, search_filter_id::SearchFilterId};
use search_filter_dynamodb::search_filter_record::{SearchFilterRecord, mk_pk, mk_sk};
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

impl From<SearchFilterRecord> for UserSearchFilter {
    fn from(record: SearchFilterRecord) -> Self {
        UserSearchFilter {
            user_id: record.user_id,
            search_filter_id: record.search_filter_id,
            search_filter: SearchFilter {
                item_query: record.item_query,
                shop_name_query: record.shop_name_query,
                price_query: record
                    .price_query
                    .map(|range_query| range_query.map(MonetaryAmount::from)),
                state_query: record
                    .state_query
                    .into_iter()
                    .map(ItemState::from)
                    .collect(),
                created_query: record.created_query,
                updated_query: record.updated_query,
            },
            created: record.created,
            updated: record.updated,
        }
    }
}

impl From<UserSearchFilter> for SearchFilterRecord {
    fn from(user_search_filter: UserSearchFilter) -> Self {
        SearchFilterRecord {
            pk: mk_pk(&user_search_filter.user_id),
            sk: mk_sk(&user_search_filter.search_filter_id),
            user_id: user_search_filter.user_id,
            search_filter_id: user_search_filter.search_filter_id,
            item_query: user_search_filter.search_filter.item_query,
            shop_name_query: user_search_filter.search_filter.shop_name_query,
            price_query: user_search_filter
                .search_filter
                .price_query
                .map(|range_query| range_query.map(u64::from)),
            state_query: user_search_filter
                .search_filter
                .state_query
                .into_iter()
                .map(ItemStateRecord::from)
                .collect(),
            created_query: user_search_filter.search_filter.created_query,
            updated_query: user_search_filter.search_filter.updated_query,
            created: user_search_filter.created,
            updated: user_search_filter.updated,
        }
    }
}
