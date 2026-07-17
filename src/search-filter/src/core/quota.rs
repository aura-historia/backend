use crate::core::user_search_filter_update::UserSearchFilterUpdate;

use product::core::product_search::{ProductSearch, ProductSearchSerdeField};
use user::core::tier::UserTier;

pub trait SearchFilterQuota {
    fn search_filter_quota(&self) -> u32;
    fn search_filter_match_quota(&self) -> u32;
    fn check_search_filter_features(
        &self,
        search: &ProductSearch,
    ) -> Result<(), ProductSearchSerdeField>;
    fn check_search_filter_update_features(
        &self,
        search: &UserSearchFilterUpdate,
    ) -> Result<(), ProductSearchSerdeField>;
}

impl SearchFilterQuota for UserTier {
    fn search_filter_quota(&self) -> u32 {
        match self {
            UserTier::Free => 1,
            UserTier::Pro => 5,
            UserTier::Ultimate => u32::MAX,
        }
    }

    fn search_filter_match_quota(&self) -> u32 {
        match self {
            UserTier::Free => 10,
            UserTier::Pro => u32::MAX,
            UserTier::Ultimate => u32::MAX,
        }
    }

    fn check_search_filter_features(
        &self,
        search: &ProductSearch,
    ) -> Result<(), ProductSearchSerdeField> {
        match self {
            UserTier::Free => check_search_filter_features_free(search),
            UserTier::Pro => check_search_filter_features_pro(search),
            UserTier::Ultimate => Ok(()),
        }
    }

    fn check_search_filter_update_features(
        &self,
        search: &UserSearchFilterUpdate,
    ) -> Result<(), ProductSearchSerdeField> {
        match self {
            UserTier::Free => check_search_filter_update_features_free(search),
            UserTier::Pro => check_search_filter_update_features_pro(search),
            UserTier::Ultimate => Ok(()),
        }
    }
}

fn check_search_filter_features_free(
    search: &ProductSearch,
) -> Result<(), ProductSearchSerdeField> {
    // allow product_query, price_query, state_query
    // forbid all other scoped filters

    check_search_filter_features_pro(search)?;

    if !search.shop_name_query.is_empty() {
        return Err(ProductSearchSerdeField::ShopNameQuery);
    }
    if !search.exclude_shop_name_query.is_empty() {
        return Err(ProductSearchSerdeField::ExcludeShopNameQuery);
    }
    if !search.seller_name_query.is_empty() {
        return Err(ProductSearchSerdeField::SellerNameQuery);
    }
    if !search.exclude_seller_name_query.is_empty() {
        return Err(ProductSearchSerdeField::ExcludeSellerNameQuery);
    }
    if !search.shop_slug_id_query.is_empty() {
        return Err(ProductSearchSerdeField::ShopSlugIdQuery);
    }
    if !search.exclude_shop_slug_id_query.is_empty() {
        return Err(ProductSearchSerdeField::ExcludeShopSlugIdQuery);
    }
    if !search.seller_slug_id_query.is_empty() {
        return Err(ProductSearchSerdeField::SellerSlugIdQuery);
    }
    if !search.exclude_seller_slug_id_query.is_empty() {
        return Err(ProductSearchSerdeField::ExcludeSellerSlugIdQuery);
    }
    if !search.shop_type_query.is_empty() {
        return Err(ProductSearchSerdeField::ShopTypeQuery);
    }
    if !search.country_query.is_empty() {
        return Err(ProductSearchSerdeField::CountryQuery);
    }
    if !search.continent_query.is_empty() {
        return Err(ProductSearchSerdeField::ContinentQuery);
    }
    if search.geo_address_distance_query.is_some() {
        return Err(ProductSearchSerdeField::GeoAddressDistanceQuery);
    }
    if search.created_query.is_some() {
        return Err(ProductSearchSerdeField::CreatedQuery);
    }
    if search.updated_query.is_some() {
        return Err(ProductSearchSerdeField::UpdatedQuery);
    }
    if search.auction_start_query.is_some() {
        return Err(ProductSearchSerdeField::AuctionStartQuery);
    }
    if search.auction_end_query.is_some() {
        return Err(ProductSearchSerdeField::AuctionEndQuery);
    }

    Ok(())
}

fn check_search_filter_features_pro(search: &ProductSearch) -> Result<(), ProductSearchSerdeField> {
    if search.enhanced_search_description.is_some() {
        return Err(ProductSearchSerdeField::EnhancedSearchDescription);
    }

    Ok(())
}

fn check_search_filter_update_features_free(
    search: &UserSearchFilterUpdate,
) -> Result<(), ProductSearchSerdeField> {
    // allow product_query, price_query, state_query
    // forbid all other scoped filters

    check_search_filter_update_features_pro(search)?;

    if search.shop_name_query.is_some() {
        return Err(ProductSearchSerdeField::ShopNameQuery);
    }
    if search.exclude_shop_name_query.is_some() {
        return Err(ProductSearchSerdeField::ExcludeShopNameQuery);
    }
    if search.seller_name_query.is_some() {
        return Err(ProductSearchSerdeField::SellerNameQuery);
    }
    if search.exclude_seller_name_query.is_some() {
        return Err(ProductSearchSerdeField::ExcludeSellerNameQuery);
    }
    if search.shop_slug_id_query.is_some() {
        return Err(ProductSearchSerdeField::ShopSlugIdQuery);
    }
    if search.exclude_shop_slug_id_query.is_some() {
        return Err(ProductSearchSerdeField::ExcludeShopSlugIdQuery);
    }
    if search.seller_slug_id_query.is_some() {
        return Err(ProductSearchSerdeField::SellerSlugIdQuery);
    }
    if search.exclude_seller_slug_id_query.is_some() {
        return Err(ProductSearchSerdeField::ExcludeSellerSlugIdQuery);
    }
    if search.shop_type_query.is_some() {
        return Err(ProductSearchSerdeField::ShopTypeQuery);
    }
    if search.country_query.is_some() {
        return Err(ProductSearchSerdeField::CountryQuery);
    }
    if search.continent_query.is_some() {
        return Err(ProductSearchSerdeField::ContinentQuery);
    }
    if search.geo_address_distance_query.is_some() {
        return Err(ProductSearchSerdeField::GeoAddressDistanceQuery);
    }
    if search.created_query.is_some() {
        return Err(ProductSearchSerdeField::CreatedQuery);
    }
    if search.updated_query.is_some() {
        return Err(ProductSearchSerdeField::UpdatedQuery);
    }
    if search.auction_start_query.is_some() {
        return Err(ProductSearchSerdeField::AuctionStartQuery);
    }
    if search.auction_end_query.is_some() {
        return Err(ProductSearchSerdeField::AuctionEndQuery);
    }

    Ok(())
}

fn check_search_filter_update_features_pro(
    search: &UserSearchFilterUpdate,
) -> Result<(), ProductSearchSerdeField> {
    if search.enhanced_search_description.is_some() {
        return Err(ProductSearchSerdeField::EnhancedSearchDescription);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::UserTier;
    use crate::core::quota::SearchFilterQuota;
    use product::core::product_search::{ProductSearch, ProductSearchSerdeField};

    #[test]
    fn should_enforce_search_filter_quota() {
        assert_eq!(UserTier::Free.search_filter_quota(), 1);
        assert_eq!(UserTier::Pro.search_filter_quota(), 5);
        assert_eq!(UserTier::Ultimate.search_filter_quota(), u32::MAX);
    }

    #[test]
    fn should_enforce_search_filter_match_quota() {
        assert_eq!(UserTier::Free.search_filter_match_quota(), 10);
        assert_eq!(UserTier::Pro.search_filter_match_quota(), u32::MAX);
        assert_eq!(UserTier::Ultimate.search_filter_match_quota(), u32::MAX);
    }

    #[test]
    fn should_forbid_country_query_when_free_tier() {
        use common::currency::domain::Currency;
        use common::language::domain::Language;
        use isocountry::CountryCode;

        let search = ProductSearch::new(Language::En, Currency::Eur)
            .with_country_query([CountryCode::DEU].into_iter().collect());

        assert_eq!(
            UserTier::Free.check_search_filter_features(&search),
            Err(ProductSearchSerdeField::CountryQuery)
        );
    }

    #[test]
    fn should_forbid_continent_query_when_free_tier() {
        use common::currency::domain::Currency;
        use common::language::domain::Language;
        use geo::core::continent::Continent;

        let search = ProductSearch::new(Language::En, Currency::Eur)
            .with_continent_query([Continent::Europe].into_iter().collect());

        assert_eq!(
            UserTier::Free.check_search_filter_features(&search),
            Err(ProductSearchSerdeField::ContinentQuery)
        );
    }

    #[test]
    fn should_forbid_geo_address_distance_query_when_free_tier() {
        use common::currency::domain::Currency;
        use common::distance::domain::{Distance, DistanceUnit, GeoDistanceQuery};
        use common::language::domain::Language;

        let geo_query = GeoDistanceQuery {
            lat: 52.52,
            lon: 13.405,
            distance: Distance {
                amount: 100.0,
                unit: DistanceUnit::Kilometers,
            },
        };
        let search = ProductSearch::new(Language::En, Currency::Eur)
            .with_geo_address_distance_query(geo_query);

        assert_eq!(
            UserTier::Free.check_search_filter_features(&search),
            Err(ProductSearchSerdeField::GeoAddressDistanceQuery)
        );
    }
}
