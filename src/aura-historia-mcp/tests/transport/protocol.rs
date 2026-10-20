use super::*;

#[tokio::test]
async fn should_discover_current_protocol_and_keep_business_catalogue_empty_by_default() {
    let app = router(config(), Registry::default(), Arc::new(AnonymousGate)).unwrap();
    let response = app
        .clone()
        .oneshot(request(&modern("server/discover", json!({}))))
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!("application/json", response.headers()["content-type"]);
    assert_eq!("no-store", response.headers()["cache-control"]);
    assert!(!response.headers().contains_key("mcp-session-id"));
    let result = json_response(response).await;
    assert_eq!(
        json!(["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"]),
        result["result"]["supportedVersions"]
    );
    let response = app
        .oneshot(request(&modern("tools/list", json!({}))))
        .await
        .unwrap();
    assert_eq!(json!([]), json_response(response).await["result"]["tools"]);
}

#[tokio::test]
async fn should_call_injected_operation_with_request_and_correlation_metadata() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut request = request(&modern(
        "tools/call",
        json!({"name":"testOperation", "arguments":{"value":"hello"}}),
    ));
    request
        .headers_mut()
        .insert("x-correlation-id", HeaderValue::from_static("upstream-123"));
    let response = app(calls.clone()).oneshot(request).await.unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!("upstream-123", response.headers()["x-correlation-id"]);
    let id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let body = json_response(response).await;
    assert_eq!(
        json!({"value":"hello", "principal":"anonymous", "request_id":id, "correlation_id":"upstream-123"}),
        body["result"]["structuredContent"]
    );
    assert_eq!(1, calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_initialize_and_call_every_supported_legacy_revision_without_a_session() {
    for version in ["2025-11-25", "2025-06-18", "2025-03-26"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = app(calls.clone());
        let initialize = json!({"jsonrpc":"2.0", "id":0, "method":"initialize", "params":{"protocolVersion":version, "capabilities":{}, "clientInfo":{"name":"legacy", "version":"1"}}});
        let response = app.clone().oneshot(request(&initialize)).await.unwrap();
        assert_eq!(StatusCode::OK, response.status(), "{version}");
        assert!(!response.headers().contains_key("mcp-session-id"));
        assert_eq!(
            version,
            json_response(response).await["result"]["protocolVersion"]
        );
        let initialized = json!({"jsonrpc":"2.0", "method":"notifications/initialized"});
        let response = app.clone().oneshot(request(&initialized)).await.unwrap();
        assert_eq!(StatusCode::ACCEPTED, response.status());
        assert!(
            to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .is_empty()
        );
        let mut call = request(
            &json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"testOperation", "arguments":{"value":"legacy"}}}),
        );
        call.headers_mut()
            .insert("mcp-protocol-version", version.parse().unwrap());
        call.headers_mut().insert(
            "mcp-session-id",
            HeaderValue::from_static("stale-ignored-session"),
        );
        let response = app.oneshot(call).await.unwrap();
        assert_eq!(StatusCode::OK, response.status());
        assert!(!response.headers().contains_key("mcp-session-id"));
        assert_eq!(
            "legacy",
            json_response(response).await["result"]["structuredContent"]["value"]
        );
        assert_eq!(1, calls.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn should_require_current_routing_headers_and_metadata_without_executing_tools() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(calls.clone());
    for header in ["mcp-method", "mcp-name", "mcp-protocol-version"] {
        let body = modern(
            "tools/call",
            json!({"name":"testOperation", "arguments":{"value":"private"}}),
        );
        let mut call = request(&body);
        call.headers_mut().remove(header);
        assert_eq!(
            StatusCode::BAD_REQUEST,
            app.clone().oneshot(call).await.unwrap().status()
        );
    }
    let mut body = modern(
        "tools/call",
        json!({"name":"testOperation", "arguments":{"value":"private"}}),
    );
    body["params"]["_meta"]
        .as_object_mut()
        .unwrap()
        .remove("io.modelcontextprotocol/clientCapabilities");
    let response = app.oneshot(request(&body)).await.unwrap();
    assert_eq!(StatusCode::BAD_REQUEST, response.status());
    assert_eq!(0, calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_decode_base64_mcp_name_before_matching_body() {
    let mut call = request(&modern(
        "tools/call",
        json!({"name":"testOperation", "arguments":{"value":"ok"}}),
    ));
    call.headers_mut().insert(
        "mcp-name",
        HeaderValue::from_static("=?base64?dGVzdE9wZXJhdGlvbg==?="),
    );
    let response = app(Arc::new(AtomicUsize::new(0)))
        .oneshot(call)
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!(
        "ok",
        json_response(response).await["result"]["structuredContent"]["value"]
    );
}

#[tokio::test]
async fn should_return_distinct_protocol_errors_for_unknown_method_tool_arguments_and_version() {
    let app = app(Arc::new(AtomicUsize::new(0)));
    for (body, status, code) in [
        (
            modern("unknown/method", json!({})),
            StatusCode::NOT_FOUND,
            -32601,
        ),
        (
            modern("tools/call", json!({"name":"unknown", "arguments":{}})),
            StatusCode::OK,
            -32602,
        ),
        (
            modern(
                "tools/call",
                json!({"name":"testOperation", "arguments":{"value":42}}),
            ),
            StatusCode::OK,
            -32602,
        ),
    ] {
        let response = app.clone().oneshot(request(&body)).await.unwrap();
        assert_eq!(status, response.status());
        assert_eq!(code, json_response(response).await["error"]["code"]);
    }
    let mut body = modern("tools/list", json!({}));
    body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = "2099-01-01".into();
    let response = app.oneshot(request(&body)).await.unwrap();
    assert_eq!(StatusCode::BAD_REQUEST, response.status());
    let error = json_response(response).await;
    assert_eq!(-32022, error["error"]["code"]);
    assert!(
        error["error"]["data"]["supported"]
            .as_array()
            .unwrap()
            .contains(&json!("2026-07-28"))
    );
}

#[tokio::test]
async fn should_work_with_official_sdk_clients_over_native_http_in_both_lifecycle_modes() {
    use rmcp::{
        ClientLifecycleMode, ClientServiceExt,
        model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut config = config();
    config.allowed_hosts = vec![address.to_string()];
    let calls = Arc::new(AtomicUsize::new(0));
    let app = router(
        config,
        Registry::new([operation(calls.clone())]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let stop = CancellationToken::new();
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
        let tools = client.list_tools(None).await.unwrap();
        assert_eq!("testOperation", tools.tools[0].name);
        let result = client
            .call_tool(
                CallToolRequestParams::new("testOperation")
                    .with_arguments(serde_json::from_value(json!({"value":"sdk-client"})).unwrap()),
            )
            .await
            .unwrap();
        assert_eq!(
            "sdk-client",
            serde_json::to_value(result).unwrap()["structuredContent"]["value"]
        );
        client.cancel().await.unwrap();
    }
    assert_eq!(2, calls.load(Ordering::SeqCst));
    stop.cancel();
    server.await.unwrap();
}

#[tokio::test]
async fn should_reject_known_sdk_versions_outside_the_selected_compatibility_matrix() {
    let app = app(Arc::new(AtomicUsize::new(0)));
    let mut call = request(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}));
    call.headers_mut().insert(
        "mcp-protocol-version",
        HeaderValue::from_static("2024-11-05"),
    );
    let response = app.oneshot(call).await.unwrap();
    assert_eq!(StatusCode::BAD_REQUEST, response.status());
    assert_eq!(-32022, json_response(response).await["error"]["code"]);
}
