use super::{PublicServices, registry};
use crate::{AnonymousGate, ConfigError, RegistryError, TransportConfig, router};
use aura_historia_runtime::{
    ProductListingConfig, ProductListingDependencies, ProductListingProviderError, RotatingRuntime,
    Runtime, RuntimeError,
};
use axum::Router;
use platform_lambda_bootstrap::VersionedCompositionCache;
use std::sync::Arc;

pub fn public_router(
    transport: TransportConfig,
    listings: &ProductListingConfig,
    runtime: &Runtime,
) -> Result<Router, PublicStartupError> {
    let dependencies = ProductListingDependencies::from_env(listings)?;
    let registry = registry(PublicServices::from_runtime(runtime, dependencies))?;
    Ok(router(transport, registry, Arc::new(AnonymousGate))?)
}

/// Credential refresh precedes every invocation, including discovery and warm catalogue calls.
/// Cached routers own their service handles; replacing a version leaves in-flight handles alive.
pub struct RotatingPublicRouter {
    runtime: RotatingRuntime,
    routers: VersionedCompositionCache<Router>,
    transport: TransportConfig,
    listings: ProductListingConfig,
}

impl RotatingPublicRouter {
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

    pub async fn current(&self) -> Result<Router, PublicStartupError> {
        let runtime = self.runtime.current().await?;
        let app = self
            .routers
            .get_or_try_build(runtime.version_id(), || async {
                public_router(self.transport.clone(), &self.listings, runtime.value())
            })
            .await?;
        Ok(app.value().clone())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PublicStartupError {
    #[error("public runtime unavailable")]
    Runtime(#[from] RuntimeError),
    #[error("public listing providers unavailable")]
    Providers(#[from] ProductListingProviderError),
    #[error("invalid public operation registry")]
    Registry(#[from] RegistryError),
    #[error("invalid MCP transport configuration")]
    Transport(#[from] ConfigError),
}
