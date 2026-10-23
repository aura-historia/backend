use aura_historia_contracts::values::{LocalizedTextData, PriceData, ProductListingPriceData};
use aura_historia_mcp::{
    AccessClass, AnonymousGate, OperationDefinition, OperationError, OperationPolicy, Registry,
    TransportConfig, router,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Flattened<T> {
    #[serde(flatten)]
    data: T,
}

async fn raw_call(app: Router, name: &str, arguments: &str) -> Value {
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
    );
    let response = app
        .oneshot(
            Request::post("/v1")
                .header("host", "mcp.example.test")
                .header("content-type", "application/json")
                .header("accept", "application/json,text/event-stream")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn should_preserve_raw_fractional_numbers_across_flattened_shared_contracts() {
    use aura_historia_contracts::{
        party_locations::management::CreateData,
        product_listings::search::ProductListingSearchData, saved_searches::CreateSearchFilterData,
    };
    let app = app(vec![
        OperationDefinition::new(
            "testLocation",
            "Location",
            policy(),
            |input: Flattened<CreateData>, _| async move {
                Ok::<_, OperationError>(input.data.position.unwrap())
            },
        )
        .unwrap(),
        OperationDefinition::new(
            "testSearch",
            "Search",
            policy(),
            |input: Flattened<ProductListingSearchData>, _| async move {
                Ok::<_, OperationError>(input.data.geography.unwrap())
            },
        )
        .unwrap(),
        OperationDefinition::new(
            "testSavedSearch",
            "Saved search",
            policy(),
            |input: Flattened<CreateSearchFilterData>, _| async move {
                let command = input
                    .data
                    .into_command(user_core::user_id::UserId::new())
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                Ok::<_, OperationError>(
                    aura_historia_contracts::geographic_search::GeographicSearchData::from(
                        command.search.geography.unwrap(),
                    ),
                )
            },
        )
        .unwrap(),
    ]);
    let geography = r#"{"origin":{"latitude":52.125,"longitude":13.375},"radius":{"amount":0.25,"unit":"KILOMETERS"},"mode":"APPROXIMATE"}"#;
    for (name, input, pointer) in [
        ("testLocation", r#"{"label":"Store","position":{"latitudeDegrees":52.125,"longitudeDegrees":13.375,"precision":"PREMISES","accuracyMetres":0.25}}"#.to_owned(), "/latitudeDegrees"),
        ("testSearch", format!(r#"{{"geography":{geography}}}"#), "/origin/latitude"),
        ("testSavedSearch", format!(r#"{{"name":"Saved","search":{{"geography":{geography}}}}}"#), "/origin/latitude"),
    ] {
        let response = raw_call(app.clone(), name, &input).await;
        let output = &response["result"]["structuredContent"];
        assert_eq!(output.pointer(pointer), Some(&json!(52.125)), "{name}: {response}");
        let longitude = if name == "testLocation" { "/longitudeDegrees" } else { "/origin/longitude" };
        let distance = if name == "testLocation" { "/accuracyMetres" } else { "/radius/amount" };
        assert_eq!(output.pointer(longitude), Some(&json!(13.375)), "{name}: {response}");
        assert_eq!(output.pointer(distance), Some(&json!(0.25)), "{name}: {response}");
        let duplicate = input.replacen("52.125", if name == "testLocation" { "52.125,\"latitudeDegrees\":53.125" } else { "52.125,\"latitude\":53.125" }, 1);
        assert_eq!(raw_call(app.clone(), name, &duplicate).await["error"]["code"], -32602);
        let forged = input.replacen("52.125", r#"{"$serde_json::private::Number":"52.125"}"#, 1);
        assert_eq!(raw_call(app.clone(), name, &forged).await["error"]["code"], -32602);
    }
}

fn policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Public,
        required_scopes: vec![],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}
async fn post(app: Router, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"shared-contract-test","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut request = Request::builder()
        .method("POST")
        .uri("/v1")
        .header("host", "mcp.example.test")
        .header("accept", "application/json,text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-method", method)
        .header("mcp-protocol-version", "2026-07-28");
    if let Some(name) = name {
        request = request.header("mcp-name", name);
    }
    let response = app
        .oneshot(
            request
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(200, response.status());
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}
fn app(operations: Vec<OperationDefinition>) -> Router {
    router(
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        Registry::new(operations).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap()
}

#[tokio::test]
async fn should_reuse_real_shared_contract_serde_and_directional_schemas_through_mcp() {
    let operation = OperationDefinition::new(
        "testLocalizedText",
        "Shared contract test",
        policy(),
        |input: LocalizedTextData, _| async move { Ok::<_, OperationError>(input) },
    )
    .unwrap();
    let app = app(vec![operation]);
    let list = post(app.clone(), "tools/list", json!({})).await;
    let tool = &list["result"]["tools"][0];
    let input_validator = jsonschema::validator_for(&tool["inputSchema"]).unwrap();
    let output_validator = jsonschema::validator_for(&tool["outputSchema"]).unwrap();
    let aliased = json!({"text":"A shared contract","language":"en-US"});
    assert!(input_validator.is_valid(&aliased));
    assert!(!output_validator.is_valid(&aliased));
    let response = post(
        app.clone(),
        "tools/call",
        json!({"name":"testLocalizedText","arguments":aliased}),
    )
    .await;
    let output = &response["result"]["structuredContent"];
    assert_eq!(json!({"text":"A shared contract","language":"en"}), *output);
    assert!(output_validator.is_valid(output));
    assert!(
        output.get("result").is_none(),
        "object DTO must retain its own shape"
    );
    let invalid = post(
        app,
        "tools/call",
        json!({"name":"testLocalizedText","arguments":{"text":"private","language":"xx"}}),
    )
    .await;
    assert_eq!(-32602, invalid["error"]["code"]);
    assert!(!invalid.to_string().contains("private"));
}

#[tokio::test]
async fn should_preserve_shared_price_integer_precision_without_transport_field_remapping() {
    let operation = OperationDefinition::new(
        "testPrice",
        "Shared price test",
        policy(),
        |input: PriceData, _| async move { Ok::<_, OperationError>(input) },
    )
    .unwrap();
    let app = app(vec![operation]);
    let list = post(app.clone(), "tools/list", json!({})).await;
    let output_schema = &list["result"]["tools"][0]["outputSchema"];
    let price = json!({"currency":"EUR","amount":u64::MAX});
    let response = post(
        app,
        "tools/call",
        json!({"name":"testPrice","arguments":price}),
    )
    .await;
    let output = &response["result"]["structuredContent"];
    assert_eq!(price, *output);
    assert!(jsonschema::is_valid(output_schema, output));
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(price, serde_json::from_str::<Value>(text).unwrap());
}

#[tokio::test]
async fn should_reuse_object_only_tagged_shared_contracts_without_an_envelope() {
    let operation = OperationDefinition::new(
        "testListingPrice",
        "Shared tagged price test",
        policy(),
        |input: ProductListingPriceData, _| async move { Ok::<_, OperationError>(input) },
    )
    .expect("each tagged variant has an object wire shape");
    let app = app(vec![operation]);
    let list = post(app.clone(), "tools/list", json!({})).await;
    let tool = &list["result"]["tools"][0];
    assert_eq!("object", tool["inputSchema"]["type"]);
    assert_eq!("object", tool["outputSchema"]["type"]);
    for price in [
        json!({"type":"ON_REQUEST"}),
        json!({"type":"MONETARY","currency":"EUR","amount":u64::MAX}),
    ] {
        assert!(jsonschema::is_valid(&tool["inputSchema"], &price));
        let response = post(
            app.clone(),
            "tools/call",
            json!({"name":"testListingPrice","arguments":price}),
        )
        .await;
        let output = &response["result"]["structuredContent"];
        assert_eq!(price, *output);
        assert!(jsonschema::is_valid(&tool["outputSchema"], output));
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(price, serde_json::from_str::<Value>(text).unwrap());
    }
}
