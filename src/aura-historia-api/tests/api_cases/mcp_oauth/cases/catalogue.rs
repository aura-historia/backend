use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_publish_the_disjoint_manifests_as_the_actual_production_registry() {
    let mcp = McpServer::start().await;
    let public: Value = serde_json::from_str(include_str!(
        "../../../../../aura-historia-mcp/public-operations.json"
    ))
    .unwrap();
    let protected: Value = serde_json::from_str(include_str!(
        "../../../../../aura-historia-mcp/authenticated-operations.json"
    ))
    .unwrap();
    let mut expected = BTreeMap::new();
    for manifest in [&public, &protected] {
        assert_eq!(MCP_RESOURCE, manifest["endpoint"]);
        for entry in manifest["operations"].as_array().unwrap() {
            let name = entry["name"].as_str().unwrap();
            assert_eq!(entry["name"], entry["operationId"]);
            assert!(
                expected.insert(name, (entry, manifest)).is_none(),
                "duplicate operation: {name}"
            );
        }
    }
    let tools = mcp.tools().await;
    assert_eq!(
        expected.keys().copied().collect::<BTreeSet<_>>(),
        tools.iter().map(|t| t["name"].as_str().unwrap()).collect()
    );
    assert_eq!(expected.len(), tools.len());
    for tool in &tools {
        let name = tool["name"].as_str().unwrap();
        let (entry, manifest) = expected[name];
        let policy = |key: &str| entry.get(key).unwrap_or(&manifest[key]).clone();
        assert_eq!(name, tool["_meta"]["aura.historia/operationId"]);
        assert_eq!(policy("access"), tool["_meta"]["aura.historia/access"]);
        assert_eq!(
            policy("requiredScopes"),
            tool["_meta"]["aura.historia/requiredScopes"]
        );
        assert_eq!(policy("annotations"), tool["annotations"]);
        jsonschema::validator_for(&tool["inputSchema"]).unwrap();
        jsonschema::validator_for(&tool["outputSchema"]).unwrap();
    }
    assert_shared_schemas(&tools);
}

fn assert_shared_schemas(tools: &[Value]) {
    use aura_historia_contracts::public_overview::PublicOverviewData;
    use aura_historia_contracts::users::account::{OwnUserData, PatchOwnUserData};
    use schemars::{JsonSchema, generate::SchemaSettings};
    fn schema<T: JsonSchema>(settings: SchemaSettings) -> Value {
        serde_json::to_value(settings.into_generator().into_root_schema_for::<T>()).unwrap()
    }
    let get = tools
        .iter()
        .find(|t| t["name"] == "getUserAccount")
        .unwrap();
    let update = tools
        .iter()
        .find(|t| t["name"] == "updateUserAccount")
        .unwrap();
    assert_eq!(
        schema::<OwnUserData>(SchemaSettings::default().for_serialize()),
        get["outputSchema"]
    );
    assert_eq!(get["outputSchema"], update["outputSchema"]);
    assert_eq!(
        schema::<PatchOwnUserData>(SchemaSettings::default().for_deserialize()),
        update["inputSchema"]
    );
    let overview = tools
        .iter()
        .find(|t| t["name"] == "getPublicOverview")
        .unwrap();
    assert_eq!(
        schema::<PublicOverviewData>(SchemaSettings::default().for_serialize()),
        overview["outputSchema"]
    );
}
