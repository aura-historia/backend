//! Shared, transport-independent construction of selected application service families.
//!
//! This composition root owns concrete pools and adapter wiring. Callers select explicit
//! typed families; transport state, DTO mapping, authentication and business behavior stay
//! with their existing owners. Extract further families only when a transport needs them.

mod postgres;
mod public_listing_source_read_budget;
mod public_listing_sources;
mod public_product_listing_history;
mod public_product_listings;
mod rotation;

pub use postgres::PostgresConfigError;
pub use public_listing_source_read_budget::PublicListingSourceReadBudget;
pub use public_listing_sources::PublicListingSources;
pub use public_product_listing_history::PublicProductListingHistory;
pub use public_product_listings::{ProductListingDependencies, PublicProductListings};
pub use rotation::{RotatingRuntime, RuntimeLease};

use sqlx::PgPool;
use std::sync::OnceLock;

/// One warm composition. Selecting a family constructs its handlers once for this pool.
pub struct Runtime {
    postgres: PgPool,
    public_listing_sources: OnceLock<PublicListingSources>,
    public_product_listing_history: OnceLock<PublicProductListingHistory>,
    public_product_listings: OnceLock<PublicProductListings>,
    listing_shipping: OnceLock<product_listing_service::shipping::ShippingEnrichment>,
}

impl Runtime {
    pub fn from_pool(postgres: PgPool) -> Self {
        Self {
            postgres,
            public_listing_sources: OnceLock::new(),
            public_product_listing_history: OnceLock::new(),
            public_product_listings: OnceLock::new(),
            listing_shipping: OnceLock::new(),
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

    pub fn public_listing_sources(&self) -> &PublicListingSources {
        self.public_listing_sources
            .get_or_init(|| PublicListingSources::compose(&self.postgres))
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
