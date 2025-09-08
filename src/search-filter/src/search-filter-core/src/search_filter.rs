use crate::{any_of_query::AnyOfQuery, range_query::RangeQuery, text_query::TextQuery};
use common::currency::domain::Currency;
use common::item_state::domain::ItemState;
use common::language::domain::Language;
use common::price::domain::MonetaryAmount;
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct SearchFilter {
    pub language: Language,
    pub currency: Currency,
    pub item_query: TextQuery,
    pub shop_name_query: Option<TextQuery>,
    pub price_query: Option<RangeQuery<MonetaryAmount>>,
    pub state_query: AnyOfQuery<ItemState>,
    pub created_query: Option<RangeQuery<OffsetDateTime>>,
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
}
