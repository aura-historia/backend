//! Shared, transport-independent construction of selected application service families.
//!
//! This composition root owns concrete pools and adapter wiring. Callers select explicit
//! typed families; transport state, DTO mapping, authentication and business behavior stay
//! with their existing owners. Extract further families only when a transport needs them.

mod access_token_authentication;
mod notifications;
mod postgres;
mod private_addresses;
pub mod product_listing_config;
mod product_listing_providers;
mod public_auctions;
mod public_listing_source_read_budget;
mod public_listing_sources;
mod public_overview;
mod public_party_locations;
mod public_product_listing_history;
mod public_product_listings;
mod rotation;
mod saved_searches;
mod shipping_management;
mod user_profile;
mod watchlist;

pub use notifications::Notifications;
pub use postgres::PostgresConfigError;
pub use private_addresses::PrivateAddresses;
pub use product_listing_config::ProductListingConfig;
pub use product_listing_providers::ProductListingProviderError;
pub use public_auctions::PublicAuctions;
pub use public_listing_source_read_budget::PublicListingSourceReadBudget;
pub use public_listing_sources::PublicListingSources;
pub use public_overview::PublicOverview;
pub use public_party_locations::PublicPartyLocations;
pub use public_product_listing_history::PublicProductListingHistory;
pub use public_product_listings::{ProductListingDependencies, PublicProductListings};
pub use rotation::{RotatingRuntime, RuntimeLease};
pub use saved_searches::SavedSearches;
pub use shipping_management::ShippingManagement;
pub use user_profile::UserProfile;
pub use watchlist::Watchlist;

use sqlx::PgPool;
use std::sync::{Arc, OnceLock};
use user_service::use_cases::AuthenticateAccessTokenUseCase;

/// One warm composition. Selecting a family constructs its handlers once for this pool.
pub struct Runtime {
    postgres: PgPool,
    notifications: OnceLock<Notifications>,
    user_profile: OnceLock<UserProfile>,
    private_addresses: OnceLock<PrivateAddresses>,
    access_token_authentication: OnceLock<Arc<dyn AuthenticateAccessTokenUseCase>>,
    public_listing_sources: OnceLock<PublicListingSources>,
    public_product_listing_history: OnceLock<PublicProductListingHistory>,
    public_auctions: OnceLock<PublicAuctions>,
    public_product_listings: OnceLock<PublicProductListings>,
    listing_shipping: OnceLock<product_listing_service::shipping::ShippingEnrichment>,
    public_party_locations: OnceLock<PublicPartyLocations>,
    public_overview: OnceLock<PublicOverview>,
    saved_searches: OnceLock<SavedSearches>,
    watchlist: OnceLock<Watchlist>,
    shipping_management: OnceLock<ShippingManagement>,
}

impl Runtime {
    pub fn from_pool(postgres: PgPool) -> Self {
        Self {
            postgres,
            notifications: OnceLock::new(),
            user_profile: OnceLock::new(),
            private_addresses: OnceLock::new(),
            access_token_authentication: OnceLock::new(),
            public_listing_sources: OnceLock::new(),
            public_product_listing_history: OnceLock::new(),
            public_auctions: OnceLock::new(),
            public_product_listings: OnceLock::new(),
            listing_shipping: OnceLock::new(),
            public_party_locations: OnceLock::new(),
            public_overview: OnceLock::new(),
            saved_searches: OnceLock::new(),
            watchlist: OnceLock::new(),
            shipping_management: OnceLock::new(),
        }
    }

    /// Native startup with the existing explicit PostgreSQL environment configuration.
    pub async fn from_env() -> Result<Self, RuntimeError> {
        let config = postgres::pool_config_from_env()?;
        let pool = config.connect().await.map_err(RuntimeError::Pool)?;
        Ok(Self::from_pool(pool))
    }

    /// Shared pool for composition of families that have not been extracted yet.
    /// Request handlers must use inbound use cases rather than this pool.
    pub fn postgres_pool(&self) -> &PgPool {
        &self.postgres
    }

    pub fn user_profile(&self) -> &UserProfile {
        self.user_profile
            .get_or_init(|| UserProfile::compose(&self.postgres))
    }

    pub fn private_addresses(&self) -> &PrivateAddresses {
        self.private_addresses
            .get_or_init(|| PrivateAddresses::compose(&self.postgres))
    }

    pub fn access_token_authentication(&self) -> &Arc<dyn AuthenticateAccessTokenUseCase> {
        self.access_token_authentication
            .get_or_init(|| access_token_authentication::compose(&self.postgres))
    }

    pub fn public_product_listings(
        &self,
        dependencies: ProductListingDependencies,
    ) -> &PublicProductListings {
        self.public_product_listings.get_or_init(|| {
            PublicProductListings::compose(
                &self.postgres,
                self.listing_shipping().clone(),
                dependencies,
            )
        })
    }

    pub fn listing_shipping(&self) -> &product_listing_service::shipping::ShippingEnrichment {
        self.listing_shipping
            .get_or_init(|| public_product_listings::shipping(&self.postgres))
    }

    pub fn public_product_listing_history(&self) -> &PublicProductListingHistory {
        self.public_product_listing_history
            .get_or_init(|| PublicProductListingHistory::compose(&self.postgres))
    }

    pub fn public_auctions(&self) -> &PublicAuctions {
        self.public_auctions.get_or_init(|| {
            PublicAuctions::compose(&self.postgres, self.listing_shipping().clone())
        })
    }

    pub fn public_listing_sources(&self) -> &PublicListingSources {
        self.public_listing_sources
            .get_or_init(|| PublicListingSources::compose(&self.postgres))
    }

    pub fn public_party_locations(&self) -> &PublicPartyLocations {
        self.public_party_locations
            .get_or_init(|| PublicPartyLocations::compose(&self.postgres))
    }

    pub fn saved_searches(
        &self,
        embeddings: Arc<dyn embedding::EmbeddingGenerator>,
    ) -> &SavedSearches {
        self.saved_searches.get_or_init(|| {
            SavedSearches::compose(&self.postgres, self.listing_shipping().clone(), embeddings)
        })
    }

    pub fn watchlist(&self) -> &Watchlist {
        self.watchlist
            .get_or_init(|| Watchlist::compose(&self.postgres, self.listing_shipping().clone()))
    }

    pub fn shipping_management(&self) -> &ShippingManagement {
        self.shipping_management
            .get_or_init(|| ShippingManagement::compose(&self.postgres))
    }

    pub fn notifications(&self) -> &Notifications {
        self.notifications
            .get_or_init(|| Notifications::compose(&self.postgres))
    }

    pub fn public_overview(&self) -> &PublicOverview {
        self.public_overview
            .get_or_init(|| PublicOverview::compose(&self.postgres))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("invalid PostgreSQL runtime configuration")]
    PostgresConfig(#[from] PostgresConfigError),
    #[error("invalid Lambda PostgreSQL runtime configuration")]
    LambdaConfig(#[from] platform_lambda_bootstrap::LambdaBootstrapConfigError),
    #[error("PostgreSQL credential provider unavailable")]
    CredentialsProvider(
        #[source] platform_postgres_secretsmanager::PostgresCredentialsProviderConfigError,
    ),
    #[error("PostgreSQL credential refresh unavailable")]
    CredentialsRefresh(#[from] platform_postgres::PostgresCredentialRefreshError),
    #[error("failed to create PostgreSQL runtime pool")]
    Pool(#[source] sqlx::Error),
}

#[cfg(test)]
mod tests;
