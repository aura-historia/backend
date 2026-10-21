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
    let registry = production_registry(runtime, dependencies)?;
    let gate = AuraTokenGate::new(
        runtime.access_token_authentication().clone(),
        transport.resource_policy,
    );
    Ok(router(transport, registry, Arc::new(gate))?)
}

fn production_registry(
    runtime: &Runtime,
    dependencies: ProductListingDependencies,
) -> Result<crate::Registry, RegistryError> {
    let saved_searches = runtime
        .saved_searches(Arc::clone(&dependencies.embeddings))
        .clone();
    let mut registered = operations(PublicServices::from_runtime(runtime, dependencies))?;
    registered.extend(crate::tools::account::operations(
        runtime.user_profile().clone(),
        runtime.private_addresses().clone(),
    )?);
    registered.extend(crate::tools::saved_searches::operations(saved_searches)?);
    registered.extend(crate::tools::watchlist::operations(
        runtime.watchlist().clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_register_saved_searches_in_the_native_and_lambda_registry() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "runtime::tests::production_registry_probe",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("OPENSEARCH_ENDPOINT_URL", "http://127.0.0.1:9")
            .env("STAGE", "ephemeral");
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
    }

    #[tokio::test]
    #[ignore = "run in an isolated process by the production registry test"]
    async fn production_registry_probe() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@127.0.0.1:9/fixture")
            .unwrap();
        let runtime = Runtime::from_pool(pool);
        let config = ProductListingConfig::from_getter(|_| None).unwrap();
        let registry = production_registry(
            &runtime,
            ProductListingDependencies::from_env(&config).unwrap(),
        )
        .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../authenticated-operations.json")).unwrap();
        for selected in manifest["operations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|selected| selected["family"] == "saved_searches")
        {
            let name = selected["operationId"].as_str().unwrap();
            assert_eq!(
                crate::AccessClass::Protected,
                registry.get(name).unwrap().policy().access
            );
        }
    }
}
