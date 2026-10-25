use crate::public::{PublicServices, operations};
use crate::{AuraTokenGate, ConfigError, RegistryError, TransportConfig, router};
use aura_historia_runtime::{
    ProductListingConfig, ProductListingDependencies, ProductListingProviderError, RotatingRuntime,
    Runtime, RuntimeError,
};
use axum::Router;
use platform_lambda_bootstrap::VersionedCompositionCache;
use std::sync::Arc;

/// Provider handles selected by the production catalogue; applications may inject local adapters.
pub struct McpDependencies {
    pub listings: ProductListingDependencies,
}

impl McpDependencies {
    pub fn from_env(listings: &ProductListingConfig) -> Result<Self, McpStartupError> {
        Ok(Self {
            listings: ProductListingDependencies::from_env(listings)?,
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
                let dependencies = McpDependencies::from_env(&self.listings)?;
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
    #[error("invalid MCP operation registry")]
    Registry(#[from] RegistryError),
    #[error("invalid MCP transport configuration")]
    Transport(#[from] ConfigError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_register_the_complete_production_catalogue() {
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
            .env("STAGE", "ephemeral")
            .env("AWS_REGION", "eu-central-1")
            .env("AWS_ACCESS_KEY_ID", "fixture")
            .env("AWS_SECRET_ACCESS_KEY", "fixture")
            .env("AWS_EC2_METADATA_DISABLED", "true");
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
    }

    #[tokio::test]
    #[ignore = "run in an isolated process by the production registry test"]
    async fn production_registry_probe() {
        let options = sqlx::postgres::PgConnectOptions::new_without_pgpass()
            .host("127.0.0.1")
            .port(9)
            .username("fixture")
            .database("fixture");
        let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy_with(options);
        let runtime = Runtime::from_pool(pool);
        let config = ProductListingConfig::from_getter(|_| None).unwrap();
        let registry =
            production_registry(&runtime, McpDependencies::from_env(&config).unwrap()).unwrap();
        let mut expected = std::collections::BTreeMap::new();
        for manifest in [
            include_str!("../public-operations.json"),
            include_str!("../authenticated-operations.json"),
        ] {
            let manifest: serde_json::Value = serde_json::from_str(manifest).unwrap();
            for entry in manifest["operations"].as_array().unwrap() {
                let name = entry["operationId"].as_str().unwrap().to_owned();
                let mut entry = entry.clone();
                for key in ["access", "requiredScopes", "annotations"] {
                    if entry.get(key).is_none() {
                        entry[key] = manifest[key].clone();
                    }
                }
                assert!(expected.insert(name, entry).is_none());
            }
        }
        let (tools, next) = registry.page(None).unwrap();
        assert_eq!(38, tools.len());
        assert!(next.is_none());
        assert_eq!(38, expected.len());
        assert_eq!(
            expected.keys().cloned().collect::<Vec<_>>(),
            tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect::<Vec<_>>()
        );
        for tool in tools {
            let entry = &expected[tool.name.as_ref()];
            let meta = serde_json::to_value(&tool.meta).unwrap();
            assert_eq!(entry["operationId"], meta["aura.historia/operationId"]);
            assert_eq!(entry["access"], meta["aura.historia/access"]);
            assert_eq!(
                entry["requiredScopes"],
                meta["aura.historia/requiredScopes"]
            );
            assert_eq!(
                entry["annotations"],
                serde_json::to_value(&tool.annotations).unwrap()
            );
            for (kind, schema) in [
                ("input", serde_json::to_value(tool.input_schema).unwrap()),
                ("output", serde_json::to_value(tool.output_schema).unwrap()),
            ] {
                assert_unsigned_bounds(&schema, &format!("{}.{kind}", tool.name));
            }
        }
    }

    fn assert_unsigned_bounds(value: &serde_json::Value, path: &str) {
        match value {
            serde_json::Value::Object(object) => {
                let bound = match object.get("format").and_then(serde_json::Value::as_str) {
                    Some("uint8") => Some(u64::from(u8::MAX)),
                    Some("uint16") => Some(u64::from(u16::MAX)),
                    Some("uint32") => Some(u64::from(u32::MAX)),
                    Some("uint64") => Some(u64::MAX),
                    Some("uint") => Some(usize::MAX as u64),
                    _ => None,
                };
                if let Some(bound) = bound {
                    let maximum = object.get("maximum").and_then(serde_json::Value::as_u64);
                    assert!(
                        maximum.is_some_and(|maximum| maximum <= bound),
                        "{path}: missing or inexact unsigned maximum: {value}"
                    );
                }
                for (key, child) in object {
                    assert_unsigned_bounds(child, &format!("{path}/{key}"));
                }
            }
            serde_json::Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    assert_unsigned_bounds(child, &format!("{path}/{index}"));
                }
            }
            _ => {}
        }
    }
}
