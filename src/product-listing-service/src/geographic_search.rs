//! Public geographic read models and evidence-aware distance display.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListingDiscoveryGeography {
    pub business: Option<DiscoveryDimension>,
    pub inventory: Option<DiscoveryDimension>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryDimension {
    pub geography: crate::geography::GeographicDimension,
    pub distance_km: Option<f64>,
}
impl ListingDiscoveryGeography {
    pub(crate) fn set_distances(
        &mut self,
        search: &product_listing_core::product_listing_search::ProductListingSearch,
    ) {
        for (dimension, query) in [
            (&mut self.business, search.business_geography.as_ref()),
            (&mut self.inventory, search.inventory_geography.as_ref()),
        ] {
            if let (Some(dimension), Some(origin)) = (dimension, query.and_then(|q| q.origin())) {
                dimension.distance_km = dimension.geography.position.map(|p| {
                    geo::core::discovery::display_distance_km(
                        geo::core::discovery::distance_km(origin, p.point()),
                        p.precision(),
                    )
                });
            }
        }
    }
    pub fn is_empty(&self) -> bool {
        self.business.is_none() && self.inventory.is_none()
    }
}
