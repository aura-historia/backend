use super::support::*;
use super::{SCHEMA, aura_integration_test};
use aura_historia_mcp::MAX_RESPONSE_BYTES;
use axum::http::StatusCode;
use rmcp::ClientLifecycleMode;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use test_api::IntegrationTestService;

#[aura_integration_test(services=[SCHEMA],isolated=true)]
async fn should_publish_exactly_thirty_eight_complete_tools_on_one_page_across_http_sdk_and_lambda()
{
    let server = Server::start(test_api::get_postgres_client().await).await;
    let expected = manifest_tools();
    assert_eq!(38, expected.len());
    let response = server.rpc(None, "tools/list", json!({})).await;
    assert_eq!(StatusCode::OK, response.status());
    let bytes = response.bytes().await.unwrap();
    assert!(bytes.len() <= MAX_RESPONSE_BYTES);
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body, server.lambda(None, "tools/list", json!({})).await);
    assert!(body["result"].get("nextCursor").is_none());
    let mut native = BTreeMap::new();
    for tool in body["result"]["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        assert!(native.insert(name.to_owned(), tool.clone()).is_none());
    }
    assert_eq!(
        expected.keys().collect::<Vec<_>>(),
        native.keys().collect::<Vec<_>>()
    );
    for (name, tool) in &native {
        let entry = &expected[name];
        assert_eq!(
            entry["operationId"],
            tool["_meta"]["aura.historia/operationId"]
        );
        assert_eq!(entry["access"], tool["_meta"]["aura.historia/access"]);
        assert_eq!(
            entry["requiredScopes"],
            tool["_meta"]["aura.historia/requiredScopes"]
        );
        assert_eq!(entry["annotations"], tool["annotations"]);
        for key in ["inputSchema", "outputSchema"] {
            assert_eq!("object", tool[key]["type"]);
            jsonschema::validator_for(&tool[key]).unwrap();
        }
    }
    for lifecycle in [discover(), ClientLifecycleMode::Initialize] {
        let client = server.client(None, lifecycle).await;
        assert_eq!(native, catalogue(&client).await);
        client.cancel().await.unwrap();
    }
}
