use crate::core::shop_type::ShopType;
use common::query::{any_of_query::AnyOfQuery, range_query::RangeQuery, text_query::TextQuery};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShopSearch {
    pub shop_name_query: Option<TextQuery<0>>,
    pub shop_type_query: AnyOfQuery<ShopType>,
    pub created: Option<RangeQuery<OffsetDateTime>>,
    pub updated: Option<RangeQuery<OffsetDateTime>>,
}
