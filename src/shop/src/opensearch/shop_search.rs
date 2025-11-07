use common::query::{range_query::RangeQuery, text_query::TextQuery};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShopSearch {
    pub shop_name_query: Option<TextQuery>,
    pub created: Option<RangeQuery<OffsetDateTime>>,
    pub updated: Option<RangeQuery<OffsetDateTime>>,
}
