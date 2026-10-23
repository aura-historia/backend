use super::support::*;
use super::{SCHEMA, aura_integration_test};
use aura_historia_mcp::MAX_RESPONSE_BYTES;
use axum::http::StatusCode;
use rmcp::ClientLifecycleMode;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use test_api::IntegrationTestService;

#[aura_integration_test(services=[SCHEMA],isolated=true)]
async fn should_publish_exactly_seventy_four_complete_tools_across_http_sdk_and_lambda_pages() {
    let server = Server::start(test_api::get_postgres_client().await).await;
    let mut expected = BTreeMap::new();
    for manifest in [
        include_str!("../../public-operations.json"),
        include_str!("../../authenticated-operations.json"),
        include_str!("../../partner-operations.json"),
    ] {
        let manifest: Value = serde_json::from_str(manifest).unwrap();
        for entry in manifest["operations"].as_array().unwrap() {
            let mut entry = entry.clone();
            for key in ["access", "requiredScopes", "annotations"] {
                if entry.get(key).is_none() {
                    entry[key] = manifest[key].clone();
                }
            }
            let name = entry["name"].as_str().unwrap().to_owned();
            assert_eq!(entry["name"], entry["operationId"]);
            assert!(expected.insert(name, entry).is_none());
        }
    }
    let partner: Value =
        serde_json::from_str(include_str!("../../partner-operations.json")).unwrap();
    assert_eq!(1, partner["exclusions"].as_array().unwrap().len());
    assert_eq!(
        "putWoocommerceListingSourceIngestionConfiguration",
        partner["exclusions"][0]["operationId"]
    );
    assert!(
        partner["exclusions"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("webhookSecret")
    );
    assert_eq!(74, expected.len());
    assert_eq!(selected_partner_names(), partner_names());
    let mut native = BTreeMap::new();
    let mut cursor = None;
    for size in [50, 24] {
        let params = cursor
            .as_ref()
            .map_or_else(|| json!({}), |c| json!({"cursor":c}));
        let response = server.rpc(None, "tools/list", params.clone()).await;
        assert_eq!(StatusCode::OK, response.status());
        let bytes = response.bytes().await.unwrap();
        assert!(bytes.len() <= MAX_RESPONSE_BYTES);
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let lambda = lambda_page(&server, params).await;
        assert_eq!(body, lambda);
        let tools = body["result"]["tools"].as_array().unwrap();
        assert_eq!(size, tools.len());
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            assert!(native.insert(name.to_owned(), tool.clone()).is_none());
        }
        cursor = body["result"]["nextCursor"].as_str().map(str::to_owned);
    }
    assert!(cursor.is_none());
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
    let response = server
        .call(
            None,
            "putWoocommerceListingSourceIngestionConfiguration",
            json!({"configuration":{"webhookSecret":"PRIVATE_EXCLUDED_SECRET"}}),
        )
        .await;
    assert_eq!(StatusCode::OK, response.status());
    let body: Value = response.json().await.unwrap();
    assert_eq!(-32602, body["error"]["code"]);
    assert!(!body.to_string().contains("PRIVATE_EXCLUDED_SECRET"));
    assert_eq!(0, server.publisher.count());
}
async fn lambda_page(server: &Server, params: Value) -> Value {
    use lambda_http::{
        lambda_runtime::{Context, LambdaEvent},
        request::LambdaRequest,
    };
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tower::ServiceExt;
    let mut params = params;
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"partner-lambda","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
    let payload:LambdaRequest=serde_json::from_value(json!({"version":"2.0","routeKey":"ANY /v1","rawPath":"/v1","rawQueryString":"","headers":{"host":server.origin.trim_start_matches("http://"),"content-type":"application/json","accept":"application/json,text/event-stream","mcp-protocol-version":"2026-07-28","mcp-method":"tools/list"},"requestContext":{"stage":"$default","http":{"method":"POST","path":"/v1","protocol":"HTTP/1.1","sourceIp":"127.0.0.1","userAgent":"partner-lambda"}},"body":json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":params}).to_string(),"isBase64Encoded":false})).unwrap();
    let app = server.app.clone();
    let service = lambda_http::service_fn(move |request| {
        aura_historia_mcp::lambda::handle_http_api_v2_request(app.clone(), request)
    });
    let mut context = Context::default();
    context.deadline = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
        + Duration::from_secs(30))
    .as_millis() as u64;
    let response = lambda_http::Adapter::from(service)
        .oneshot(LambdaEvent { payload, context })
        .await
        .unwrap();
    let response = serde_json::to_value(response).unwrap();
    assert!(serde_json::to_vec(&response).unwrap().len() < 6 * 1024 * 1024);
    assert_eq!(200, response["statusCode"]);
    assert_eq!("no-store", response["headers"]["cache-control"]);
    let bytes = response["body"].as_str().unwrap();
    assert!(bytes.len() <= MAX_RESPONSE_BYTES);
    serde_json::from_str(bytes).unwrap()
}
fn selected_partner_names() -> BTreeSet<String> {
    [
        "getMyListingSources",
        "listPartnerProductListings",
        "getPartnerProductListing",
        "updatePartnerProductListing",
        "withdrawPartnerProductListing",
        "restorePartnerProductListing",
        "createPartyLocation",
        "listPartyLocations",
        "getPartyLocation",
        "correctPartyLocation",
        "setPartyLocationLifecycle",
        "getSourceLocationDefault",
        "assignSourceLocationDefault",
        "changeSourceLocationDefaultApplicability",
        "clearSourceLocationDefault",
        "listShippingProfiles",
        "createShippingProfile",
        "getShippingProfile",
        "replaceShippingProfile",
        "retireShippingProfile",
        "restoreShippingProfile",
        "getShippingDefault",
        "assignShippingDefault",
        "clearShippingDefault",
        "postPartnerProductListings",
        "patchPartnerProductListings",
        "putPartnerProductListings",
        "deletePartnerProductListings",
        "postAsyncPartnerProductListings",
        "patchAsyncPartnerProductListings",
        "putAsyncPartnerProductListings",
        "deleteAsyncPartnerProductListings",
        "putShopifyListingSourceIngestionConfiguration",
        "getProductListingDelivery",
        "changeProductListingDelivery",
        "submitProductListingDelivery",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
