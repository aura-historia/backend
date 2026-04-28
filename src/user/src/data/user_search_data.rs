use crate::core::{role::UserRole, tier::UserTier, user_search::UserSearch};
use crate::data::{role_data::UserRoleData, tier_data::UserTierData};
use common::distance::data::GeoDistanceQueryData;
use common::query::{range_query::RangeQuery, text_query::TextQuery};
use geo::{core::continent::Continent, data::continent_data::ContinentData};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSearchData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<TextQuery<0>>,
    #[serde(rename = "email", default, skip_serializing_if = "Option::is_none")]
    pub email_query: Option<TextQuery<0>>,
    #[serde(rename = "firstName", default, skip_serializing_if = "Option::is_none")]
    pub first_name_query: Option<TextQuery<0>>,
    #[serde(rename = "lastName", default, skip_serializing_if = "Option::is_none")]
    pub last_name_query: Option<TextQuery<0>>,
    #[serde(rename = "tier", skip_serializing_if = "HashSet::is_empty", default)]
    pub tier_query: HashSet<UserTierData>,
    #[serde(rename = "role", skip_serializing_if = "HashSet::is_empty", default)]
    pub role_query: HashSet<UserRoleData>,
    #[serde(rename = "country", skip_serializing_if = "HashSet::is_empty", default)]
    pub country_query: HashSet<CountryCode>,
    #[serde(
        rename = "continent",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    pub continent_query: HashSet<ContinentData>,
    #[serde(
        rename = "geoAddress",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub geo_address_distance_query: Option<GeoDistanceQueryData>,
    #[serde(
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub created: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub updated: Option<RangeQuery<OffsetDateTime>>,
}

impl From<UserSearchData> for UserSearch {
    fn from(data: UserSearchData) -> Self {
        UserSearch {
            query: data.query,
            email_query: data.email_query,
            first_name_query: data.first_name_query,
            last_name_query: data.last_name_query,
            tier_query: data.tier_query.into_iter().map(UserTier::from).collect(),
            role_query: data.role_query.into_iter().map(UserRole::from).collect(),
            country_query: data.country_query.into(),
            continent_query: data
                .continent_query
                .into_iter()
                .map(Continent::from)
                .collect(),
            geo_address_distance_query: data.geo_address_distance_query.map(Into::into),
            created: data.created,
            updated: data.updated,
        }
    }
}

impl From<UserSearch> for UserSearchData {
    fn from(search: UserSearch) -> Self {
        UserSearchData {
            query: search.query,
            email_query: search.email_query,
            first_name_query: search.first_name_query,
            last_name_query: search.last_name_query,
            tier_query: search
                .tier_query
                .into_iter()
                .map(UserTierData::from)
                .collect(),
            role_query: search
                .role_query
                .into_iter()
                .map(UserRoleData::from)
                .collect(),
            country_query: search.country_query.into(),
            continent_query: search
                .continent_query
                .into_iter()
                .map(ContinentData::from)
                .collect(),
            geo_address_distance_query: search.geo_address_distance_query.map(Into::into),
            created: search.created,
            updated: search.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for UserSearchData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let all_codes: Vec<CountryCode> = CountryCode::iter().copied().collect();
            let n_countries = rng.random_range(0..=3usize);
            let country_query: HashSet<CountryCode> = (0..n_countries)
                .map(|_| all_codes[rng.random_range(0..all_codes.len())])
                .collect();
            UserSearchData {
                query: Faker.fake(),
                email_query: Faker.fake(),
                first_name_query: Faker.fake(),
                last_name_query: Faker.fake(),
                tier_query: config.fake_with_rng(rng),
                role_query: config.fake_with_rng(rng),
                country_query,
                continent_query: config.fake_with_rng(rng),
                geo_address_distance_query: config.fake_with_rng(rng),
                created: None,
                updated: None,
            }
        }
    }
}
