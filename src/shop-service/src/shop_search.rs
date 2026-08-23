use domain_primitives::query::{
    any_of_query::AnyOfQuery, range_query::RangeQuery, text_query::TextQuery,
};
use isocountry::CountryCode;
use shop_core::{continent::Continent, partner_status::ShopPartnerStatus, shop_type::ShopType};
use time::OffsetDateTime;

/// Application query input for the shop search use case.
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

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn should_default_to_empty_search_when_no_filters_set() {
        let search = ShopSearch::default();

        assert_eq!(None, search.shop_name_query);
        assert!(search.shop_type_query.is_empty());
        assert!(search.partner_status_query.is_empty());
        assert!(search.countries.is_empty());
        assert!(search.continents.is_empty());
        assert_eq!(None, search.created);
        assert_eq!(None, search.updated);
    }

    #[test]
    fn should_hold_all_search_filters_when_set() {
        let search = ShopSearch {
            shop_name_query: Some(TextQuery::<0>::try_from("antik").unwrap()),
            shop_type_query: [ShopType::AuctionHouse, ShopType::Marketplace]
                .into_iter()
                .collect(),
            partner_status_query: [ShopPartnerStatus::Partnered].into_iter().collect(),
            countries: [CountryCode::DEU, CountryCode::USA].into_iter().collect(),
            continents: [Continent::Europe].into_iter().collect(),
            created: Some(RangeQuery {
                min: Some(datetime!(2024-01-01 0:00 UTC)),
                max: Some(datetime!(2024-12-31 23:59:59 UTC)),
            }),
            updated: Some(RangeQuery {
                min: Some(datetime!(2025-01-01 0:00 UTC)),
                max: None,
            }),
        };

        assert_eq!(Some("antik"), search.shop_name_query.as_deref());
        assert!(search.shop_type_query.contains(&ShopType::AuctionHouse));
        assert!(search.shop_type_query.contains(&ShopType::Marketplace));
        assert!(
            search
                .partner_status_query
                .contains(&ShopPartnerStatus::Partnered)
        );
        assert!(search.countries.contains(&CountryCode::DEU));
        assert!(search.countries.contains(&CountryCode::USA));
        assert!(search.continents.contains(&Continent::Europe));
    }
}
