use crate::public::{PublicServices, operations};
use crate::{AuraTokenGate, ConfigError, RegistryError, TransportConfig, router};
use aura_historia_runtime::{
    ProductListingConfig, ProductListingDependencies, ProductListingProviderError, Runtime,
    RuntimeError,
};
use axum::Router;
use platform_lambda_bootstrap::LazyComposition;
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

/// Composes the router on the first invocation of an execution environment and serves every
/// later invocation from it. A failed composition is retried by the next invocation.
pub struct LambdaMcpRouter {
    runtime: Runtime,
    routers: LazyComposition<Router>,
    transport: TransportConfig,
    listings: ProductListingConfig,
}

impl LambdaMcpRouter {
    pub fn new(
        runtime: Runtime,
        transport: TransportConfig,
        listings: ProductListingConfig,
    ) -> Self {
        Self {
            runtime,
            routers: LazyComposition::new(),
            transport,
            listings,
        }
    }

    pub async fn current(&self) -> Result<Router, McpStartupError> {
        self.routers
            .get_or_try_build(|| async {
                let dependencies = McpDependencies::from_env(&self.listings)?;
                mcp_router(self.transport.clone(), &self.runtime, dependencies)
            })
            .await
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
        assert_eq!(37, tools.len());
        assert!(next.is_none());
        assert_directory_metadata(&tools);
        assert_described_arguments(&tools);
        assert_amount_tools_state_the_minor_unit_rule(&tools);
        assert_eq!(37, expected.len());
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

    /// The Claude and ChatGPT directories require a display title and explicit hints, and Claude
    /// flags an argument without a `type`; a nullable `anyOf` union passes its review.
    fn assert_directory_metadata(tools: &[rmcp::model::Tool]) {
        let mut titles = std::collections::BTreeSet::new();
        for tool in tools {
            let published = serde_json::to_value(tool).unwrap();
            for (name, argument) in published["inputSchema"]["properties"]
                .as_object()
                .into_iter()
                .flatten()
            {
                assert!(
                    argument.get("type").is_some() || argument.get("anyOf").is_some(),
                    "{}.{name}: argument without a type: {argument}",
                    tool.name
                );
            }
            let title = published["title"].as_str().unwrap_or_default();
            assert!(!title.is_empty(), "{}: missing title", tool.name);
            assert!(
                titles.insert(title.to_lowercase()),
                "{}: repeated title {title}",
                tool.name
            );
            for hint in [
                "readOnlyHint",
                "destructiveHint",
                "idempotentHint",
                "openWorldHint",
            ] {
                assert!(
                    published["annotations"][hint].is_boolean(),
                    "{}: missing {hint}",
                    tool.name
                );
            }
        }
    }

    /// Models load a tool's input schema, so an argument without a description is guesswork. Every
    /// property of every input schema, however deeply nested behind a `$ref`, carries a description,
    /// and so does every tagged-union variant. A property that a combinator branch only restates,
    /// for example to constrain its type, shares the description declared beside it.
    fn assert_described_arguments(tools: &[rmcp::model::Tool]) {
        let mut missing = Vec::new();
        for tool in tools {
            let schema = serde_json::to_value(&tool.input_schema).unwrap();
            collect_undescribed(&schema, &schema, &tool.name, &mut Vec::new(), &mut missing);
        }
        assert!(
            missing.is_empty(),
            "{} arguments without a description:\n{}",
            missing.len(),
            missing.join("\n")
        );
    }

    fn is_described(schema: &serde_json::Value) -> bool {
        schema
            .get("description")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|description| !description.trim().is_empty())
    }

    /// Names this schema describes at its own level: its properties, and those of the `$ref`
    /// target and combinator branches that refine it.
    fn described_names(
        schema: &serde_json::Value,
        root: &serde_json::Value,
        followed: &mut Vec<String>,
    ) -> std::collections::BTreeSet<String> {
        let mut names = std::collections::BTreeSet::new();
        let Some(object) = schema.as_object() else {
            return names;
        };
        if let Some(reference) = object.get("$ref").and_then(serde_json::Value::as_str) {
            let name = reference.trim_start_matches("#/$defs/");
            if !followed.iter().any(|followed| followed == name) {
                followed.push(name.to_owned());
                names.extend(described_names(&root["$defs"][name], root, followed));
                followed.pop();
            }
        }
        for keyword in ["allOf", "anyOf", "oneOf"] {
            for branch in object
                .get(keyword)
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                names.extend(described_names(branch, root, followed));
            }
        }
        for (name, property) in object
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flatten()
        {
            if is_described(property) {
                names.insert(name.clone());
            }
        }
        names
    }

    fn collect_undescribed(
        schema: &serde_json::Value,
        root: &serde_json::Value,
        path: &str,
        followed: &mut Vec<String>,
        missing: &mut Vec<String>,
    ) {
        let described = described_names(schema, root, &mut Vec::new());
        collect_in_level(schema, root, path, followed, missing, &described);
    }

    /// Walks one schema level: the schema, its `$ref` target and its combinator branches share
    /// `described`; every property schema starts a new level.
    fn collect_in_level(
        schema: &serde_json::Value,
        root: &serde_json::Value,
        path: &str,
        followed: &mut Vec<String>,
        missing: &mut Vec<String>,
        described: &std::collections::BTreeSet<String>,
    ) {
        let Some(object) = schema.as_object() else {
            return;
        };
        if let Some(reference) = object.get("$ref").and_then(serde_json::Value::as_str) {
            let name = reference.trim_start_matches("#/$defs/");
            if !followed.iter().any(|followed| followed == name) {
                followed.push(name.to_owned());
                collect_in_level(
                    &root["$defs"][name],
                    root,
                    path,
                    followed,
                    missing,
                    described,
                );
                followed.pop();
            }
        }
        for keyword in ["allOf", "anyOf", "oneOf"] {
            for variant in object
                .get(keyword)
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                let tag = variant["properties"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .find_map(|(_, property)| property.get("const"));
                if let Some(tag) = tag.filter(|_| !is_described(variant)) {
                    missing.push(format!("{path} (variant {tag})"));
                }
                collect_in_level(variant, root, path, followed, missing, described);
            }
        }
        if let Some(items) = object.get("items") {
            collect_in_level(items, root, path, followed, missing, described);
        }
        for (name, property) in object
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flatten()
        {
            let property_path = format!("{path}.{name}");
            if !described.contains(name) {
                missing.push(property_path.clone());
            }
            collect_undescribed(property, root, &property_path, followed, missing);
        }
    }

    use crate::registry::declares_money;

    /// Every tool that accepts or returns a money amount states how amounts are encoded, and the
    /// price and per-item shipping arguments state it themselves.
    fn assert_amount_tools_state_the_minor_unit_rule(tools: &[rmcp::model::Tool]) {
        let rule = aura_historia_contracts::wire::description::MINOR_UNIT_RULE;
        let mut with_amounts = 0;
        for tool in tools {
            let input = serde_json::to_value(&tool.input_schema).unwrap();
            let output = serde_json::to_value(&tool.output_schema).unwrap();
            if !(declares_money(&input) || declares_money(&output)) {
                continue;
            }
            with_amounts += 1;
            let description = tool.description.as_deref().unwrap_or_default();
            assert!(
                description.contains(rule),
                "{}: handles amounts without stating the minor-unit rule",
                tool.name
            );
            for (name, definition) in input["$defs"].as_object().into_iter().flatten() {
                for field in ["price", "maximumPerItem"] {
                    if let Some(property) = definition["properties"].get(field) {
                        assert!(
                            property["description"]
                                .as_str()
                                .is_some_and(|text| text.contains(rule)),
                            "{}: {name}.{field} does not state the minor-unit rule",
                            tool.name
                        );
                    }
                }
            }
            for field in ["price", "maximumPerItem"] {
                if let Some(property) = input["properties"].get(field) {
                    assert!(
                        property["description"]
                            .as_str()
                            .is_some_and(|text| text.contains(rule)),
                        "{}: {field} does not state the minor-unit rule",
                        tool.name
                    );
                }
            }
        }
        assert!(
            with_amounts >= 15,
            "only {with_amounts} tools handle amounts"
        );
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
