mod authentication;
mod fixtures;
mod rotation;
mod sdk;

use super::*;
use crate::{AnonymousGate, MAX_RESPONSE_BYTES, TransportConfig, router};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use fixtures::{Fixture, SearchOutcome};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tower::ServiceExt;

const LISTING_ID: &str = "pl_01k6k0w000e008000000000001";
const AUCTION_ID: &str = "auc_01k6k0w000e008000000000004";

fn manifest() -> Value {
    serde_json::from_str(include_str!("../../../public-operations.json")).unwrap()
}
fn search_input() -> Value {
    let mut input: Value = serde_json::from_str(include_str!(
        "../../../../aura-historia-contracts/tests/fixtures/listing-search.json"
    ))
    .unwrap();
    input.as_object_mut().unwrap().remove("searchAfter");
    input
}
fn inputs() -> Vec<(&'static str, Value)> {
    vec![
        ("simpleSearchProductListings", search_input()),
        (
            "getProductListing",
            json!({"productListingId": LISTING_ID, "language":"de-CH", "currency":"JPY"}),
        ),
        (
            "getProductListingByTitleSlug",
            json!({"productListingTitleSlugId":"cabinet-abcdef", "language":"de-CH", "currency":"JPY"}),
        ),
        (
            "getSimilarProductListings",
            json!({"productListingId": LISTING_ID}),
        ),
        (
            "getProductListingHistory",
            json!({"productListingId": LISTING_ID, "currency":"USD"}),
        ),
        ("listAuctions", json!({})),
        ("getAuction", json!({"auctionId": AUCTION_ID})),
        (
            "getAuctionCatalogue",
            json!({"auctionId": AUCTION_ID, "language":"de", "currency":"USD", "pageSize":100}),
        ),
        (
            "searchPublicListingSources",
            json!({"query":"museum", "size":50}),
        ),
        (
            "getPublicListingSourceBySlug",
            json!({"listingSourceSlugId":"source"}),
        ),
        (
            "discoverPartyLocations",
            json!({"geography":{"countries":["DE"]}, "size":100}),
        ),
        ("getPublicOverview", json!({})),
    ]
}
fn minimal_inputs() -> Vec<(&'static str, Value)> {
    vec![
        ("simpleSearchProductListings", json!({})),
        ("getProductListing", json!({"productListingId": LISTING_ID})),
        (
            "getProductListingByTitleSlug",
            json!({"productListingTitleSlugId":"cabinet-abcdef"}),
        ),
        (
            "getSimilarProductListings",
            json!({"productListingId": LISTING_ID}),
        ),
        (
            "getProductListingHistory",
            json!({"productListingId": LISTING_ID}),
        ),
        ("listAuctions", json!({})),
        ("getAuction", json!({"auctionId": AUCTION_ID})),
        ("getAuctionCatalogue", json!({"auctionId": AUCTION_ID})),
        ("searchPublicListingSources", json!({})),
        (
            "getPublicListingSourceBySlug",
            json!({"listingSourceSlugId":"source"}),
        ),
        (
            "discoverPartyLocations",
            json!({"geography":{"countries":["DE"]}}),
        ),
        ("getPublicOverview", json!({})),
    ]
}

fn app(fixture: &Arc<Fixture>) -> Router {
    router(
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        registry(fixture.services()).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap()
}
fn body(method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28", "io.modelcontextprotocol/clientInfo":{"name":"public-release-fixture","version":"1"}, "io.modelcontextprotocol/clientCapabilities":{}});
    json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params})
}
async fn post(app: Router, method: &str, params: Value) -> (StatusCode, Value, usize) {
    let body = body(method, params);
    let mut request = Request::builder()
        .method("POST")
        .uri("/v1")
        .header("host", "mcp.example.test")
        .header("content-type", "application/json")
        .header("accept", "application/json,text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", method);
    if let Some(name) = body.pointer("/params/name").and_then(Value::as_str) {
        request = request.header("mcp-name", name);
    }
    let response = app
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap(), bytes.len())
}
fn validate_output(tool: &Value, result: &Value) -> Value {
    assert_ne!(Some(true), result["isError"].as_bool(), "{result}");
    let output = result["structuredContent"].clone();
    let validator = jsonschema::validator_for(&tool["outputSchema"]).unwrap();
    assert!(
        validator.is_valid(&output),
        "{}",
        validator
            .iter_errors(&output)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(output, text);
    assert!(!output.to_string().contains("userState"));
    output
}

#[tokio::test]
async fn should_publish_exactly_the_selected_operations_with_shared_schemas_and_read_policies() {
    let fixture = Arc::new(Fixture::default());
    let (_, list, bytes) = post(app(&fixture), "tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    let manifest = manifest();
    let operations = manifest["operations"].as_array().unwrap();
    assert_eq!(12, tools.len());
    assert!(list["result"].get("nextCursor").is_none());
    let swagger = include_str!("../../../../../docs/swagger.yaml");
    for (tool, operation) in tools.iter().zip(operations) {
        let name = operation["operationId"].as_str().unwrap();
        assert_eq!(tool["name"], operation["name"]);
        assert_eq!(tool["name"], operation["operationId"]);
        assert_eq!(
            1,
            swagger
                .lines()
                .filter(|line| line.trim() == format!("operationId: {name}"))
                .count()
        );
        assert_eq!(tool["annotations"], manifest["annotations"]);
        assert_eq!(tool["_meta"]["aura.historia/access"], manifest["access"]);
        assert_eq!(
            tool["_meta"]["aura.historia/requiredScopes"],
            manifest["requiredScopes"]
        );
        assert_eq!(
            tool["_meta"]["aura.historia/operationId"],
            operation["operationId"]
        );
        jsonschema::validator_for(&tool["inputSchema"]).unwrap();
        jsonschema::validator_for(&tool["outputSchema"]).unwrap();
    }
    assert!(fixture.calls.lock().unwrap().is_empty());
    println!("public tools/list JSON bytes: {bytes}");
}

#[tokio::test]
async fn should_accept_minimal_defaulted_inputs_for_every_selected_tool() {
    let fixture = Arc::new(Fixture::default());
    let app = app(&fixture);
    let (_, listed, _) = post(app.clone(), "tools/list", json!({})).await;
    let mut mismatches = vec![];
    for (name, input) in minimal_inputs() {
        let tool = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        let validator = jsonschema::validator_for(&tool["inputSchema"]).unwrap();
        for error in validator.iter_errors(&input) {
            mismatches.push(format!("{name}: {error}"));
        }
        let (_, response, _) = post(
            app.clone(),
            "tools/call",
            json!({"name":name,"arguments":input}),
        )
        .await;
        if response.get("error").is_some() {
            mismatches.push(format!("{name}: {response}"));
        } else {
            validate_output(tool, &response["result"]);
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[tokio::test]
async fn should_reject_unselected_operations_and_invalid_arguments_before_calling_a_service() {
    let fixture = Arc::new(Fixture::default());
    let app = app(&fixture);
    for name in [
        "putNewsletterSubscription",
        "confirmNewsletterSubscription",
        "getUser",
        "createProductListings",
        "getAdminOverview",
        "health",
        "token",
        "search",
        "fetch",
    ] {
        let (_, response, _) = post(
            app.clone(),
            "tools/call",
            json!({"name":name, "arguments":{}}),
        )
        .await;
        assert_eq!(-32602, response["error"]["code"], "{name}: {response}");
    }
    let (_, response, _) = post(
        app,
        "tools/call",
        json!({"name":"getProductListing", "arguments":{"productListingId":"private-malformed"}}),
    )
    .await;
    assert_eq!(-32602, response["error"]["code"]);
    assert!(!response.to_string().contains("private-malformed"));
    assert!(fixture.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn should_expose_safe_temporary_failures_from_the_composed_search_tool() {
    let fixture = Arc::new(Fixture {
        search_outcome: SearchOutcome::Unavailable,
        ..Default::default()
    });
    let (_, response, _) = post(
        app(&fixture),
        "tools/call",
        json!({"name":"simpleSearchProductListings", "arguments":{}}),
    )
    .await;
    assert_eq!(true, response["result"]["isError"]);
    assert_eq!(
        "TEMPORARILY_UNAVAILABLE",
        response["result"]["content"][0]["text"]
    );
    assert!(!response.to_string().contains("PRIVATE_PROVIDER_SENTINEL"));
}

#[tokio::test]
async fn should_fail_explicitly_when_complete_detail_or_history_exceeds_the_actual_transport_limit()
{
    let fixture = Arc::new(Fixture {
        url_padding: 8192,
        history_count: 20_000,
        ..Default::default()
    });
    let app = app(&fixture);
    for name in ["getProductListing", "getProductListingHistory"] {
        let (_, response, bytes) = post(
            app.clone(),
            "tools/call",
            json!({"name":name, "arguments":{"productListingId":LISTING_ID}}),
        )
        .await;
        assert_eq!(-32603, response["error"]["code"], "{name}");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("transport limit")
        );
        assert!(response.get("result").is_none());
        assert!(bytes < 1024);
    }
}

#[tokio::test]
async fn should_preserve_complete_large_public_results_through_the_actual_lambda_adapter() {
    use lambda_http::lambda_runtime::{Context, LambdaEvent};
    let fixture = Arc::new(Fixture::default());
    let app = app(&fixture);
    let service = lambda_http::service_fn(move |request| {
        crate::lambda::handle_http_api_v2_request(app.clone(), request)
    });
    for name in ["getProductListing", "getProductListingHistory"] {
        let body = body(
            "tools/call",
            json!({"name":name, "arguments":{"productListingId":LISTING_ID}}),
        );
        let event = json!({"version":"2.0","routeKey":"ANY /v1","rawPath":"/dev/v1","rawQueryString":"","headers":{"host":"mcp.example.test","content-type":"application/json","accept":"application/json,text/event-stream","mcp-protocol-version":"2026-07-28","mcp-method":"tools/call","mcp-name":name},"requestContext":{"stage":"dev","http":{"method":"POST","path":"/dev/v1","protocol":"HTTP/1.1","sourceIp":"127.0.0.1","userAgent":"public-release-fixture"}},"body":body.to_string(),"isBase64Encoded":false});
        let payload: lambda_http::request::LambdaRequest = serde_json::from_value(event).unwrap();
        let mut context = Context::default();
        context.request_id = "fixture-public-lambda".into();
        context.deadline = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
            + Duration::from_secs(30))
        .as_millis() as u64;
        let response = lambda_http::Adapter::from(service.clone())
            .oneshot(LambdaEvent { payload, context })
            .await
            .unwrap();
        let bytes = serde_json::to_vec(&response).unwrap();
        assert!(bytes.len() < 6 * 1024 * 1024);
        let response: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(200, response["statusCode"]);
        let body = response["body"].as_str().unwrap();
        let value: Value = serde_json::from_str(body).unwrap();
        let result = &value["result"]["structuredContent"];
        let count = match name {
            "getProductListing" => result["item"]["images"].as_array().unwrap().len(),
            _ => result["result"].as_array().unwrap().len(),
        };
        assert_eq!(
            if name == "getProductListing" {
                175
            } else {
                128
            },
            count
        );
        println!(
            "{name}: complete JSON body {} bytes, Lambda envelope {} bytes",
            body.len(),
            bytes.len()
        );
    }
}
