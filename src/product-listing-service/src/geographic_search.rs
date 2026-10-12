//! Public listing geography and evidence-aware distance display.
#[derive(Debug, Clone, PartialEq)]
pub struct ListingDiscoveryGeography {
    pub geography: crate::geography::GeographicDimension,
    pub distance_km: Option<f64>,
}
impl ListingDiscoveryGeography {
    pub(crate) fn set_distance(
        &mut self,
        search: &product_listing_core::product_listing_search::ProductListingSearch,
    ) {
        self.distance_km = search
            .geography
            .as_ref()
            .and_then(|q| q.origin())
            .and_then(|origin| {
                self.geography.position.map(|p| {
                    geo::core::discovery::display_distance_km(
                        geo::core::discovery::distance_km(origin, p.point()),
                        p.precision(),
                    )
                })
            });
    }
}
