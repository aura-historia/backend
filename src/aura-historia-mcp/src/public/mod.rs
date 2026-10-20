//! The selected anonymous catalogue. Family modules own transport mappings and use-case calls.
mod runtime;
use crate::{
    Registry, RegistryError, auctions, product_listing_history, product_listings, public_discovery,
};
use aura_historia_runtime::{
    ProductListingDependencies, PublicAuctions, PublicListingSourceReadBudget,
    PublicListingSources, PublicOverview, PublicPartyLocations, PublicProductListingHistory,
    PublicProductListings, Runtime,
};
pub use runtime::{PublicStartupError, RotatingPublicRouter, public_router};

/// Typed handles allow native/Lambda composition and isolated protocol fixtures to use one registry.
pub struct PublicServices {
    pub listings: PublicProductListings,
    pub history: PublicProductListingHistory,
    pub auctions: PublicAuctions,
    pub sources: PublicListingSources,
    pub source_budget: PublicListingSourceReadBudget,
    pub locations: PublicPartyLocations,
    pub overview: PublicOverview,
}

impl PublicServices {
    pub fn from_runtime(
        runtime: &Runtime,
        listing_dependencies: ProductListingDependencies,
    ) -> Self {
        Self {
            listings: runtime
                .public_product_listings(listing_dependencies)
                .clone(),
            history: runtime.public_product_listing_history().clone(),
            auctions: runtime.public_auctions().clone(),
            sources: runtime.public_listing_sources().clone(),
            source_budget: PublicListingSourceReadBudget::default(),
            locations: runtime.public_party_locations().clone(),
            overview: runtime.public_overview().clone(),
        }
    }
}

/// Registers exactly the public release families; private, write and control-plane operations
/// have no registration path here. The committed manifest is checked against this catalogue.
pub fn registry(services: PublicServices) -> Result<Registry, RegistryError> {
    let mut operations = product_listings::operations(services.listings)?;
    operations.extend(product_listing_history::operations(services.history)?);
    operations.extend(auctions::operations(services.auctions)?);
    operations.extend(public_discovery::operations(
        services.sources,
        services.source_budget,
        services.locations,
        services.overview,
    )?);
    Registry::new(operations)
}

#[cfg(test)]
mod tests;
