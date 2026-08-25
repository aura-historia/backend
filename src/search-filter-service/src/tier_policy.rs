use product_listing_core::product_listing_search::ProductListingSearch;
use user_core::tier::UserTier;

pub(crate) fn active_filter_quota(tier: UserTier) -> usize {
    match tier {
        UserTier::Free => 1,
        UserTier::Pro => 5,
        UserTier::Ultimate => usize::MAX,
    }
}

pub(crate) fn monthly_match_quota(tier: UserTier) -> usize {
    match tier {
        UserTier::Free => 10,
        UserTier::Pro | UserTier::Ultimate => usize::MAX,
    }
}

pub(crate) fn validate_search_features(
    tier: UserTier,
    search: &ProductListingSearch,
) -> Result<(), &'static str> {
    restricted_feature(tier, search).map_or(Ok(()), Err)
}

pub(crate) fn validate_search_feature_changes(
    tier: UserTier,
    before: &ProductListingSearch,
    after: &ProductListingSearch,
) -> Result<(), &'static str> {
    for feature in restricted_features(tier) {
        if feature.is_present(after) && !feature.is_present(before) {
            return Err(feature.name());
        }
    }

    Ok(())
}

fn restricted_feature(tier: UserTier, search: &ProductListingSearch) -> Option<&'static str> {
    restricted_features(tier)
        .iter()
        .find(|feature| feature.is_present(search))
        .map(|feature| feature.name())
}

fn restricted_features(tier: UserTier) -> &'static [RestrictedFeature] {
    match tier {
        UserTier::Free => &FREE_RESTRICTED_FEATURES,
        UserTier::Pro => &PRO_RESTRICTED_FEATURES,
        UserTier::Ultimate => &[],
    }
}

#[derive(Clone, Copy)]
enum RestrictedFeature {
    EnhancedSearchDescription,
    ShopNameQuery,
    ExcludeShopNameQuery,
    SellerNameQuery,
    ExcludeSellerNameQuery,
    ShopSlugIdQuery,
    ExcludeShopSlugIdQuery,
    SellerSlugIdQuery,
    ExcludeSellerSlugIdQuery,
    ShopTypeQuery,
    CountryQuery,
    ContinentQuery,
    GeoAddressDistanceQuery,
    CreatedQuery,
    UpdatedQuery,
    AuctionStartQuery,
    AuctionEndQuery,
}

const PRO_RESTRICTED_FEATURES: [RestrictedFeature; 1] =
    [RestrictedFeature::EnhancedSearchDescription];
const FREE_RESTRICTED_FEATURES: [RestrictedFeature; 17] = [
    RestrictedFeature::EnhancedSearchDescription,
    RestrictedFeature::ShopNameQuery,
    RestrictedFeature::ExcludeShopNameQuery,
    RestrictedFeature::SellerNameQuery,
    RestrictedFeature::ExcludeSellerNameQuery,
    RestrictedFeature::ShopSlugIdQuery,
    RestrictedFeature::ExcludeShopSlugIdQuery,
    RestrictedFeature::SellerSlugIdQuery,
    RestrictedFeature::ExcludeSellerSlugIdQuery,
    RestrictedFeature::ShopTypeQuery,
    RestrictedFeature::CountryQuery,
    RestrictedFeature::ContinentQuery,
    RestrictedFeature::GeoAddressDistanceQuery,
    RestrictedFeature::CreatedQuery,
    RestrictedFeature::UpdatedQuery,
    RestrictedFeature::AuctionStartQuery,
    RestrictedFeature::AuctionEndQuery,
];

impl RestrictedFeature {
    fn name(self) -> &'static str {
        match self {
            Self::EnhancedSearchDescription => "enhancedSearchDescription",
            Self::ShopNameQuery => "shopNameQuery",
            Self::ExcludeShopNameQuery => "excludeShopNameQuery",
            Self::SellerNameQuery => "sellerNameQuery",
            Self::ExcludeSellerNameQuery => "excludeSellerNameQuery",
            Self::ShopSlugIdQuery => "shopSlugIdQuery",
            Self::ExcludeShopSlugIdQuery => "excludeShopSlugIdQuery",
            Self::SellerSlugIdQuery => "sellerSlugIdQuery",
            Self::ExcludeSellerSlugIdQuery => "excludeSellerSlugIdQuery",
            Self::ShopTypeQuery => "shopTypeQuery",
            Self::CountryQuery => "countryQuery",
            Self::ContinentQuery => "continentQuery",
            Self::GeoAddressDistanceQuery => "geoAddressDistanceQuery",
            Self::CreatedQuery => "createdQuery",
            Self::UpdatedQuery => "updatedQuery",
            Self::AuctionStartQuery => "auctionStartQuery",
            Self::AuctionEndQuery => "auctionEndQuery",
        }
    }

    fn is_present(self, search: &ProductListingSearch) -> bool {
        match self {
            Self::EnhancedSearchDescription => search.enhanced_search_description.is_some(),
            Self::ShopNameQuery => !search.shop_name_query.is_empty(),
            Self::ExcludeShopNameQuery => !search.exclude_shop_name_query.is_empty(),
            Self::SellerNameQuery => !search.seller_name_query.is_empty(),
            Self::ExcludeSellerNameQuery => !search.exclude_seller_name_query.is_empty(),
            Self::ShopSlugIdQuery => !search.shop_slug_id_query.is_empty(),
            Self::ExcludeShopSlugIdQuery => !search.exclude_shop_slug_id_query.is_empty(),
            Self::SellerSlugIdQuery => !search.seller_slug_id_query.is_empty(),
            Self::ExcludeSellerSlugIdQuery => !search.exclude_seller_slug_id_query.is_empty(),
            Self::ShopTypeQuery => !search.shop_type_query.is_empty(),
            Self::CountryQuery => !search.country_query.is_empty(),
            Self::ContinentQuery => !search.continent_query.is_empty(),
            Self::GeoAddressDistanceQuery => search.geo_address_distance_query.is_some(),
            Self::CreatedQuery => search.created_query.is_some(),
            Self::UpdatedQuery => search.updated_query.is_some(),
            Self::AuctionStartQuery => search.auction_start_query.is_some(),
            Self::AuctionEndQuery => search.auction_end_query.is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        active_filter_quota, monthly_match_quota, validate_search_feature_changes,
        validate_search_features,
    };
    use isocountry::CountryCode;
    use localization::Language;
    use money::Currency;
    use product_listing_core::{
        listing_availability::ListingAvailability,
        product_listing_id::ProductListingId,
        product_listing_search::{EnhancedSearchDescription, ProductListingSearch},
    };
    use std::collections::HashSet;
    use user_core::tier::UserTier;

    #[test]
    fn should_apply_active_filter_quotas_by_tier() {
        assert_eq!(1, active_filter_quota(UserTier::Free));
        assert_eq!(5, active_filter_quota(UserTier::Pro));
        assert_eq!(usize::MAX, active_filter_quota(UserTier::Ultimate));
        assert_eq!(10, monthly_match_quota(UserTier::Free));
        assert_eq!(usize::MAX, monthly_match_quota(UserTier::Pro));
    }

    #[test]
    fn should_reject_tier_restricted_search_features() {
        let country_search = ProductListingSearch::new(Language::En, Currency::Eur)
            .with_country_query([CountryCode::DEU].into_iter().collect());
        let enhanced_search = ProductListingSearch::new(Language::En, Currency::Eur)
            .with_enhanced_search_description(
                EnhancedSearchDescription::try_from("gold ring").unwrap(),
            );

        assert_eq!(
            Err("countryQuery"),
            validate_search_features(UserTier::Free, &country_search)
        );
        assert_eq!(
            Err("enhancedSearchDescription"),
            validate_search_features(UserTier::Pro, &enhanced_search)
        );
        assert_eq!(
            Ok(()),
            validate_search_features(UserTier::Ultimate, &enhanced_search)
        );
    }

    #[test]
    fn should_allow_free_tier_product_exclusions_and_availability_filters() {
        let with_product_exclusion = ProductListingSearch::new(Language::En, Currency::Eur)
            .with_exclude_product_listing_id_query(HashSet::from([ProductListingId::new()]).into());
        let with_availability = ProductListingSearch::new(Language::En, Currency::Eur)
            .with_availability_query(HashSet::from([ListingAvailability::SoldOut]).into());

        assert_eq!(
            Ok(()),
            validate_search_features(UserTier::Free, &with_product_exclusion)
        );
        assert_eq!(
            Ok(()),
            validate_search_features(UserTier::Free, &with_availability)
        );
    }

    #[test]
    fn should_allow_edits_to_plan_restricted_filter_until_reactivation() {
        let restricted = ProductListingSearch::new(Language::En, Currency::Eur)
            .with_country_query([CountryCode::DEU].into_iter().collect());
        let unchanged = restricted.clone();
        let mut adds_restriction = ProductListingSearch::new(Language::En, Currency::Eur);
        adds_restriction.country_query = [CountryCode::DEU].into_iter().collect();

        assert_eq!(
            Ok(()),
            validate_search_feature_changes(UserTier::Free, &restricted, &unchanged)
        );
        assert_eq!(
            Err("countryQuery"),
            validate_search_features(UserTier::Free, &restricted),
            "reactivation must recheck the stored full search"
        );
        assert_eq!(
            Err("countryQuery"),
            validate_search_feature_changes(
                UserTier::Free,
                &ProductListingSearch::new(Language::En, Currency::Eur),
                &adds_restriction,
            )
        );
    }
}
