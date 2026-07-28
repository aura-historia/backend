use crate::continent::Continent;
use crate::partner_status::ShopPartnerStatus;
use crate::shop_type::ShopType;
use common::query::{any_of_query::AnyOfQuery, range_query::RangeQuery, text_query::TextQuery};
use isocountry::CountryCode;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShopSearch {
    pub shop_name_query: Option<TextQuery<0>>,
    pub shop_type_query: AnyOfQuery<ShopType>,
    pub partner_status_query: AnyOfQuery<ShopPartnerStatus>,
    pub countries: AnyOfQuery<CountryCode>,
    pub continents: AnyOfQuery<Continent>,
    pub created: Option<RangeQuery<OffsetDateTime>>,
    pub updated: Option<RangeQuery<OffsetDateTime>>,
}
