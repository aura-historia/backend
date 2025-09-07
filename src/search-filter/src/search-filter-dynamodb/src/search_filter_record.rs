use common::user_id::UserId;
use item_dynamodb::item_state_record::ItemStateRecord;
use search_filter_core::{
    range_query::RangeQuery, search_filter_id::SearchFilterId, text_query::TextQuery,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFilterRecord {
    pub pk: String,

    pub sk: String,

    pub user_id: UserId,

    pub search_filter_id: SearchFilterId,

    pub item_query: TextQuery,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_name_query: Option<TextQuery>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_query: Option<RangeQuery<u64>>,

    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub state_query: HashSet<ItemStateRecord>,

    #[serde(
        with = "search_filter_core::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub created_query: Option<RangeQuery<OffsetDateTime>>,

    #[serde(
        with = "search_filter_core::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}
