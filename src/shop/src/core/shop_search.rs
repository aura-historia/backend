use crate::core::shop_type::ShopType;
use common::query::{any_of_query::AnyOfQuery, range_query::RangeQuery, text_query::TextQuery};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShopSearch {
    pub shop_name_query: Option<TextQuery<0>>,
    pub shop_type_query: AnyOfQuery<ShopType>,
    pub created: Option<RangeQuery<OffsetDateTime>>,
    pub updated: Option<RangeQuery<OffsetDateTime>>,
    /// Minimum relevance score threshold for search results.
    /// Results with scores below this threshold will be filtered out.
    /// Typically ranges from 0.0 to higher values depending on the query.
    /// When None, all matching results are returned regardless of score.
    pub min_score: Option<f64>,
}
