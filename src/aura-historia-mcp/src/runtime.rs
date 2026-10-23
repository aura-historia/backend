use crate::public::{PublicServices, operations};
use crate::{AuraTokenGate, ConfigError, RegistryError, TransportConfig, router};
use aura_historia_runtime::{
    ProductListingConfig, ProductListingDependencies, ProductListingIngestionConfigError,
    ProductListingIngestionDependencies, ProductListingProviderError, RotatingRuntime, Runtime,
    RuntimeError,
};
use axum::Router;
use platform_lambda_bootstrap::VersionedCompositionCache;
use std::sync::Arc;

/// Provider handles selected by the production catalogue; applications may inject local adapters.
pub struct McpDependencies {
    pub listings: ProductListingDependencies,
    pub ingestion: ProductListingIngestionDependencies,
}

impl McpDependencies {
    pub async fn from_env(listings: &ProductListingConfig) -> Result<Self, McpStartupError> {
        Ok(Self {
            listings: ProductListingDependencies::from_env(listings)?,
            ingestion: ProductListingIngestionDependencies::from_env().await?,
        })
    }
}

pub fn mcp_router(
    transport: TransportConfig,
    runtime: &Runtime,
    dependencies: McpDependencies,
) -> Result<Router, McpStartupError> {
    let registry = production_registry(runtime, dependencies)?;
    let gate = AuraTokenGate::new(
        runtime.access_token_authentication().clone(),
        transport.resource_policy,
    );
    Ok(router(transport, registry, Arc::new(gate))?)
}

fn production_registry(
    runtime: &Runtime,
    dependencies: McpDependencies,
) -> Result<crate::Registry, RegistryError> {
    let saved_searches = runtime
        .saved_searches(Arc::clone(&dependencies.listings.embeddings))
        .clone();
    let mut registered = operations(PublicServices::from_runtime(runtime, dependencies.listings))?;
    registered.extend(crate::tools::account::operations(
        runtime.user_profile().clone(),
        runtime.private_addresses().clone(),
    )?);
    registered.extend(crate::tools::saved_searches::operations(saved_searches)?);
    registered.extend(crate::tools::watchlist::operations(
        runtime.watchlist().clone(),
    )?);
    registered.extend(crate::tools::notifications::operations(
        runtime.notifications().clone(),
    )?);
    let ingestion = runtime.partner_product_listing_ingestion(dependencies.ingestion);
    registered.extend(crate::tools::partner_listing_management::operations(
        runtime.partner_listing_management().clone(),
    )?);
    registered.extend(crate::tools::partner_product_listing_batches::operations(
        runtime.partner_product_listing_batches().clone(),
    )?);
    registered.extend(crate::tools::partner_product_listing_ingestion::operations(
        ingestion.clone(),
    )?);
    registered.extend(crate::tools::party_locations::operations(
        runtime.party_locations().clone(),
    )?);
    registered.extend(crate::tools::source_location_defaults::operations(
        runtime.source_location_defaults().clone(),
    )?);
    registered.extend(crate::tools::shipping::operations(
        runtime.shipping_management().clone(),
    )?);
    registered.extend(crate::tools::product_listing_delivery::operations(
        runtime.product_listing_delivery().clone(),
        ingestion.submit.clone(),
    )?);
    crate::Registry::new(registered)
}

/// Credential refresh precedes every invocation, including discovery and warm catalogue calls.
/// Cached routers own their service handles; replacing a version leaves in-flight handles alive.
pub struct RotatingMcpRouter {
    runtime: RotatingRuntime,
    routers: VersionedCompositionCache<Router>,
    transport: TransportConfig,
    listings: ProductListingConfig,
}

impl RotatingMcpRouter {
    pub fn new(
        runtime: RotatingRuntime,
        transport: TransportConfig,
        listings: ProductListingConfig,
    ) -> Self {
        Self {
            runtime,
            routers: VersionedCompositionCache::new(),
            transport,
            listings,
        }
    }

    pub async fn current(&self) -> Result<Router, McpStartupError> {
        let runtime = self.runtime.current().await?;
        let app = self
            .routers
            .get_or_try_build(runtime.version_id(), || async {
                let dependencies = McpDependencies::from_env(&self.listings).await?;
                mcp_router(self.transport.clone(), runtime.value(), dependencies)
            })
            .await?;
        Ok(app.value().clone())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpStartupError {
    #[error("MCP runtime unavailable")]
    Runtime(#[from] RuntimeError),
    #[error("MCP listing providers unavailable")]
    Providers(#[from] ProductListingProviderError),
    #[error("MCP ingestion publisher unavailable")]
    Ingestion(#[from] ProductListingIngestionConfigError),
    #[error("invalid MCP operation registry")]
    Registry(#[from] RegistryError),
    #[error("invalid MCP transport configuration")]
    Transport(#[from] ConfigError),
}

#[cfg(test)]
mod tests;
