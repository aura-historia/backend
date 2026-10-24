use super::*;
use aura_historia_contracts::product_listings::search::ProductListingSearchOptionsData;
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, RoleClient,
    model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};

type Client = RunningService<RoleClient, ClientConfig>;

#[tokio::test]
async fn should_call_all_twelve_public_tools_and_continue_large_results_with_official_sdk_clients()
{
    let fixture = Arc::new(Fixture::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = router(
        TransportConfig {
            allowed_hosts: vec![address.to_string()],
            ..Default::default()
        },
        registry(fixture.services()).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    let shutdown = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap();
    });
    for lifecycle in [
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
        ClientLifecycleMode::Initialize,
    ] {
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/v1")),
        );
        let mut config = ClientConfig::default();
        config.protocol_version = ProtocolVersion::V_2025_11_25;
        let client = tokio::time::timeout(
            Duration::from_secs(5),
            config.serve_with_lifecycle(transport, lifecycle),
        )
        .await
        .unwrap()
        .unwrap();
        exercise_tools(&client, &fixture).await;
        client.cancel().await.unwrap();
    }
    let called: BTreeSet<_> = fixture.calls.lock().unwrap().iter().copied().collect();
    assert_eq!(
        inputs()
            .into_iter()
            .map(|(name, _)| name)
            .collect::<BTreeSet<_>>(),
        called
    );
    stop.cancel();
    server.await.unwrap();
}

async fn exercise_tools(client: &Client, fixture: &Arc<Fixture>) {
    let tools: Value = serde_json::to_value(client.list_tools(None).await.unwrap()).unwrap();
    assert_eq!(12, tools["tools"].as_array().unwrap().len());
    for (name, input) in inputs() {
        let tool = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        let validator = jsonschema::validator_for(&tool["inputSchema"]).unwrap();
        assert!(
            validator.is_valid(&input),
            "{name}: {}",
            validator
                .iter_errors(&input)
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
        let result = invoke(client, name, input.clone()).await;
        let output = validate_output(tool, &result);
        assert_complete_payload(name, &result, &output);
        if name == "simpleSearchProductListings" {
            continue_search(client, fixture, tool, input, output).await;
        }
    }
}

async fn invoke(client: &Client, name: &'static str, input: Value) -> Value {
    let result = client
        .call_tool(
            CallToolRequestParams::new(name).with_arguments(serde_json::from_value(input).unwrap()),
        )
        .await
        .unwrap();
    serde_json::to_value(result).unwrap()
}

fn assert_complete_payload(name: &str, result: &Value, output: &Value) {
    match name {
        "simpleSearchProductListings" => {
            assert_eq!(100, output["items"].as_array().unwrap().len());
            assert!(serde_json::to_vec(result).unwrap().len() > 64 * 1024);
            assert_eq!(
                3,
                output["items"][99]["item"]["images"]
                    .as_array()
                    .unwrap()
                    .len()
            );
        }
        "getProductListing" | "getProductListingByTitleSlug" => {
            assert_eq!(175, output["item"]["images"].as_array().unwrap().len());
            assert!(output["item"]["images"][174]["url"].as_str().unwrap().len() > 512);
        }
        "getProductListingHistory" => {
            assert_eq!(128, output["result"].as_array().unwrap().len());
            assert!(output.get("searchAfter").is_none());
            assert_ne!(
                output["result"][1]["payload"]["changes"][0]["current"]["display"]["fxRateId"],
                output["result"][127]["payload"]["changes"][0]["current"]["display"]["fxRateId"]
            );
        }
        "getSimilarProductListings" => assert_eq!("EMBEDDING_PENDING", output["status"]),
        _ => {}
    }
}

async fn continue_search(
    client: &Client,
    fixture: &Arc<Fixture>,
    tool: &Value,
    mut input: Value,
    output: Value,
) {
    input["searchAfter"] = output["searchAfter"].clone();
    let result = invoke(client, "simpleSearchProductListings", input).await;
    let next = validate_output(tool, &result);
    assert_eq!(1, next["items"].as_array().unwrap().len());
    assert!(next.get("searchAfter").is_none());
    let calls = fixture.searches.lock().unwrap();
    let last = calls.last().unwrap();
    let options: ProductListingSearchOptionsData = serde_json::from_value(search_input()).unwrap();
    let expected = options
        .into_request(serde_json::from_value(search_input()).unwrap())
        .unwrap();
    assert_eq!(expected.search, last.search);
    assert_eq!(expected.sort, last.sort);
    let cursor = last.cursor.as_ref().unwrap();
    assert_eq!(100, cursor.size);
    assert_eq!(
        serde_json::to_value(u64::MAX).unwrap(),
        cursor.search_after.as_ref().unwrap().search_after[0]
    );
}
