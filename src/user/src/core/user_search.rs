use crate::core::{role::UserRole, tier::UserTier};
use common::distance::domain::GeoDistanceQuery;
use common::query::{any_of_query::AnyOfQuery, range_query::RangeQuery, text_query::TextQuery};
use geo::core::continent::Continent;
use isocountry::CountryCode;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UserSearch {
    pub query: Option<TextQuery<0>>,
    pub email_query: Option<TextQuery<0>>,
    pub first_name_query: Option<TextQuery<0>>,
    pub last_name_query: Option<TextQuery<0>>,
    pub tier_query: AnyOfQuery<UserTier>,
    pub role_query: AnyOfQuery<UserRole>,
    pub country_query: AnyOfQuery<CountryCode>,
    pub continent_query: AnyOfQuery<Continent>,
    pub geo_address_distance_query: Option<GeoDistanceQuery>,
    pub created: Option<RangeQuery<OffsetDateTime>>,
    pub updated: Option<RangeQuery<OffsetDateTime>>,
}
