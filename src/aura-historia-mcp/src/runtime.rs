use crate::public::{PublicServices, operations};
use crate::{AuraTokenGate, ConfigError, RegistryError, TransportConfig, router};
use aura_historia_runtime::{
    ProductListingConfig, ProductListingDependencies, ProductListingProviderError, RotatingRuntime,
    Runtime, RuntimeError,
};
use axum::Router;
use platform_lambda_bootstrap::VersionedCompositionCache;
use std::sync::Arc;

pub fn mcp_router(
    transport: TransportConfig,
    listings: &ProductListingConfig,
    runtime: &Runtime,
) -> Result<Router, McpStartupError> {
    let dependencies = ProductListingDependencies::from_env(listings)?;
    let mut definitions = operations(PublicServices::from_runtime(runtime, dependencies))?;
    definitions.extend(crate::tools::notifications::operations(
        runtime.notifications().clone(),
    )?);
    let registry = crate::Registry::new(definitions)?;
    let gate = AuraTokenGate::new(
        runtime.access_token_authentication().clone(),
        transport.resource_policy,
    );
    Ok(router(transport, registry, Arc::new(gate))?)
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
                mcp_router(self.transport.clone(), &self.listings, runtime.value())
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
    #[error("invalid MCP operation registry")]
    Registry(#[from] RegistryError),
    #[error("invalid MCP transport configuration")]
    Transport(#[from] ConfigError),
}
