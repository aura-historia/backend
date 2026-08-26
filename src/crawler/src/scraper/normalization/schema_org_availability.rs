//! schema.org availability translation at the crawler anti-corruption boundary.

use crate::scraper::normalization::listing_availability_mapping::ListingAvailabilityMapping;
use product_listing_core::listing_availability::ListingAvailability;

/// Never expose schema.org vocabulary outside this adapter.
pub(crate) fn map_schema_org_availability(value: &str) -> Option<ListingAvailabilityMapping> {
    let value = value.trim();
    let tail = value.rsplit('/').next().unwrap_or(value);
    let mapping = match tail {
        "InStock" => ListingAvailabilityMapping::Availability(ListingAvailability::InStock),
        "LimitedAvailability" => {
            ListingAvailabilityMapping::Availability(ListingAvailability::LimitedAvailability)
        }
        "BackOrder" => ListingAvailabilityMapping::Availability(ListingAvailability::BackOrder),
        "MadeToOrder" => ListingAvailabilityMapping::Availability(ListingAvailability::MadeToOrder),
        "PreOrder" => ListingAvailabilityMapping::Availability(ListingAvailability::PreOrder),
        "PreSale" => ListingAvailabilityMapping::Availability(ListingAvailability::PreSale),
        "Reserved" => ListingAvailabilityMapping::Availability(ListingAvailability::Reserved),
        "OutOfStock" => ListingAvailabilityMapping::Availability(ListingAvailability::OutOfStock),
        "SoldOut" => ListingAvailabilityMapping::Availability(ListingAvailability::SoldOut),
        // Channel/catalog policy is not lifecycle evidence.
        "OnlineOnly" | "InStoreOnly" | "Discontinued" => ListingAvailabilityMapping::NoAssertion,
        _ if value.contains("schema.org/") => ListingAvailabilityMapping::Ignore,
        _ => return None,
    };
    Some(mapping)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_only_supported_schema_org_availability_values() {
        assert_eq!(
            map_schema_org_availability("https://schema.org/OutOfStock"),
            Some(ListingAvailabilityMapping::Availability(
                ListingAvailability::OutOfStock
            ))
        );
        assert_eq!(
            map_schema_org_availability("https://schema.org/Reserved"),
            Some(ListingAvailabilityMapping::Availability(
                ListingAvailability::Reserved
            ))
        );
    }

    #[test]
    fn should_not_turn_schema_org_catalog_policy_into_lifecycle() {
        assert_eq!(
            map_schema_org_availability("https://schema.org/Discontinued"),
            Some(ListingAvailabilityMapping::NoAssertion)
        );
        assert_eq!(
            map_schema_org_availability("https://schema.org/UnknownAvailability"),
            Some(ListingAvailabilityMapping::Ignore)
        );
    }
}
