use super::*;

#[tokio::test]
async fn should_reject_header_body_disagreement_before_admission_or_business_work() {
    let admitted = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = router(
        config(),
        Registry::new([operation(calls.clone())]).unwrap(),
        Arc::new(CountingGate(admitted.clone())),
    )
    .unwrap();
    let body = modern(
        "tools/call",
        json!({"name":"testOperation", "arguments":{"value":"private"}}),
    );
    for (name, value) in [
        ("mcp-method", "tools/list"),
        ("mcp-name", "otherOperation"),
        ("mcp-protocol-version", "2025-11-25"),
    ] {
        let mut call = request(&body);
        call.headers_mut().insert(name, value.parse().unwrap());
        let response = app.clone().oneshot(call).await.unwrap();
        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(-32020, json_response(response).await["error"]["code"]);
    }
    let mut legacy = request(
        &json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"testOperation", "arguments":{"value":"private"}}}),
    );
    legacy
        .headers_mut()
        .insert("mcp-name", HeaderValue::from_static("otherOperation"));
    assert_eq!(
        StatusCode::BAD_REQUEST,
        app.clone().oneshot(legacy).await.unwrap().status()
    );
    let mut duplicate = request(&body);
    duplicate
        .headers_mut()
        .append("mcp-name", HeaderValue::from_static("testOperation"));
    assert_eq!(
        StatusCode::BAD_REQUEST,
        app.oneshot(duplicate).await.unwrap().status()
    );
    assert_eq!(0, admitted.load(Ordering::SeqCst));
    assert_eq!(0, calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_reject_present_unapproved_origins_and_preserve_exact_ports() {
    let mut config = config();
    config.allowed_origins = vec!["https://app.example.test".into()];
    let app = router(config, Registry::default(), Arc::new(AnonymousGate)).unwrap();
    for (origin, expected) in [
        ("https://app.example.test", StatusCode::OK),
        ("https://app.example.test:443", StatusCode::OK),
        ("https://app.example.test:444", StatusCode::FORBIDDEN),
        ("null", StatusCode::FORBIDDEN),
        ("https://evil.test", StatusCode::FORBIDDEN),
        ("https://app.example.test/path", StatusCode::FORBIDDEN),
    ] {
        let mut call = request(&modern("tools/list", json!({})));
        call.headers_mut().insert("origin", origin.parse().unwrap());
        assert_eq!(
            expected,
            app.clone().oneshot(call).await.unwrap().status(),
            "{origin}"
        );
    }
}

#[tokio::test]
async fn should_reject_bad_host_oversized_malformed_and_batched_json_before_business_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(calls.clone());
    let body = modern(
        "tools/call",
        json!({"name":"testOperation", "arguments":{"value":"private"}}),
    );
    let mut call = request(&body);
    call.headers_mut()
        .insert("host", HeaderValue::from_static("attacker.test"));
    assert_eq!(
        StatusCode::FORBIDDEN,
        app.clone().oneshot(call).await.unwrap().status()
    );
    for (raw, expected) in [
        ("x".repeat(256 * 1024 + 1), StatusCode::PAYLOAD_TOO_LARGE),
        ("{private malformed".into(), StatusCode::BAD_REQUEST),
        (json!([body]).to_string(), StatusCode::BAD_REQUEST),
    ] {
        let mut call = request(&json!({}));
        *call.body_mut() = Body::from(raw);
        let response = app.clone().oneshot(call).await.unwrap();
        assert_eq!(expected, response.status());
        assert!(
            !json_response(response)
                .await
                .to_string()
                .contains("private")
        );
    }
    assert_eq!(0, calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_reject_http_content_negotiation_and_unavailable_stream_routes() {
    let app = app(Arc::new(AtomicUsize::new(0)));
    for (header, value, expected) in [
        (
            "content-type",
            "application/jsonx",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        ("accept", "application/json", StatusCode::NOT_ACCEPTABLE),
        (
            "accept",
            "application/json;q=0,text/event-stream",
            StatusCode::NOT_ACCEPTABLE,
        ),
    ] {
        let mut call = request(&modern("tools/list", json!({})));
        call.headers_mut().insert(header, value.parse().unwrap());
        assert_eq!(expected, app.clone().oneshot(call).await.unwrap().status());
    }
    for method in ["GET", "DELETE"] {
        let mut call = request(&json!({}));
        *call.method_mut() = method.parse().unwrap();
        let response = app.clone().oneshot(call).await.unwrap();
        assert_eq!(StatusCode::METHOD_NOT_ALLOWED, response.status());
        assert_eq!("POST", response.headers()["allow"]);
    }
}

#[tokio::test]
async fn should_fail_closed_for_supplied_credentials_and_leave_protected_tools_discoverable() {
    let mut protected = policy();
    protected.access = AccessClass::Protected;
    protected.required_scopes = vec![CredentialCapability::UsersRead];
    let op = OperationDefinition::new(
        "privateOperation",
        "Private",
        protected,
        |_: Arguments, _| async { Ok(json!({})) },
    )
    .unwrap();
    let app = router(
        config(),
        Registry::new([op]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(request(&modern("tools/list", json!({}))))
        .await
        .unwrap();
    let tool = &json_response(response).await["result"]["tools"][0];
    assert_eq!("privateOperation", tool["name"]);
    assert_eq!(
        json!(["users:read"]),
        tool["_meta"]["aura.historia/requiredScopes"]
    );
    assert_eq!(
        json!({"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}),
        tool["annotations"]
    );
    let response = app
        .clone()
        .oneshot(request(&modern(
            "tools/call",
            json!({"name":"privateOperation","arguments":{"value":"private"}}),
        )))
        .await
        .unwrap();
    assert_eq!(StatusCode::UNAUTHORIZED, response.status());
    let mut call = request(&modern("tools/list", json!({})));
    call.headers_mut()
        .insert("authorization", HeaderValue::from_static("Bearer secret"));
    let response = app.oneshot(call).await.unwrap();
    assert_eq!(StatusCode::UNAUTHORIZED, response.status());
    assert_eq!("no-store", response.headers()["cache-control"]);
}

#[tokio::test]
async fn should_challenge_before_sdk_dispatch_and_preserve_exact_delegated_capabilities() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let op = OperationDefinition::new("testOperation","Test",policy(), move |_: Arguments, context| {
        observed.fetch_add(1,Ordering::SeqCst);
        async move {
            assert!(matches!(context.operation.principal,Principal::DelegatedUser { capabilities, .. } if capabilities == BTreeSet::from([CredentialCapability::UsersRead])));
            Ok(json!({"allowed":true}))
        }
    }).unwrap();
    let app = router(
        config(),
        Registry::new([op]).unwrap(),
        Arc::new(DelegatedGate),
    )
    .unwrap();
    let body = modern(
        "tools/call",
        json!({"name":"testOperation","arguments":{"value":"private"}}),
    );
    let mut malformed = body.clone();
    malformed["params"]["arguments"] = json!("private malformed arguments");
    for body in [&body, &malformed] {
        let response = app.clone().oneshot(request(body)).await.unwrap();
        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert!(response.headers().contains_key("www-authenticate"));
        assert_eq!("no-store", response.headers()["cache-control"]);
        assert!(
            to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(0, calls.load(Ordering::SeqCst));
    let mut call = request(&body);
    call.headers_mut().insert(
        "authorization",
        HeaderValue::from_static("Bearer test-only"),
    );
    let response = app.oneshot(call).await.unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!(1, calls.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn should_cancel_operation_at_deadline_and_when_request_is_dropped() {
    let request_timeout = Duration::from_millis(40);
    for (disconnect, expected_elapsed) in [(false, request_timeout), (true, Duration::ZERO)] {
        let started = Arc::new(tokio::sync::Notify::new());
        let token = Arc::new(tokio::sync::Mutex::new(None));
        let observed_token = token.clone();
        let observed_started = started.clone();
        let op = OperationDefinition::new(
            "testOperation",
            "Test",
            policy(),
            move |_: Arguments, context| {
                let token = observed_token.clone();
                let started = observed_started.clone();
                async move {
                    *token.lock().await = Some(context.cancellation);
                    started.notify_one();
                    std::future::pending::<Result<Value, OperationError>>().await
                }
            },
        )
        .unwrap();
        let mut config = config();
        config.request_timeout = request_timeout;
        let app = router(
            config,
            Registry::new([op]).unwrap(),
            Arc::new(AnonymousGate),
        )
        .unwrap();
        let task = tokio::spawn(app.oneshot(request(&modern(
            "tools/call",
            json!({"name":"testOperation","arguments":{"value":"pending"}}),
        ))));
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("operation did not start before its deadline");
        let cancellation: CancellationToken = token.lock().await.clone().unwrap();
        assert!(!cancellation.is_cancelled());
        let started_at = tokio::time::Instant::now();
        if disconnect {
            task.abort();
            assert!(
                tokio::time::timeout(Duration::from_secs(1), task)
                    .await
                    .expect("aborted request did not stop")
                    .unwrap_err()
                    .is_cancelled()
            );
        } else {
            tokio::time::advance(request_timeout).await;
            assert_eq!(
                StatusCode::REQUEST_TIMEOUT,
                tokio::time::timeout(Duration::from_secs(1), task)
                    .await
                    .expect("request did not finish at its deadline")
                    .unwrap()
                    .unwrap()
                    .status()
            );
        }
        tokio::time::timeout(Duration::from_secs(1), cancellation.cancelled())
            .await
            .expect("request did not cancel the operation");
        assert_eq!(expected_elapsed, started_at.elapsed());
    }
}

#[tokio::test]
async fn should_cancel_operation_when_native_http_connection_is_closed() {
    use std::io::Write;

    let started = Arc::new(tokio::sync::Notify::new());
    let observed = Arc::new(tokio::sync::Mutex::new(None));
    let observed_token = observed.clone();
    let observed_started = started.clone();
    let op = OperationDefinition::new(
        "testOperation",
        "Test",
        policy(),
        move |_: Arguments, context| {
            let observed = observed_token.clone();
            let started = observed_started.clone();
            async move {
                *observed.lock().await = Some(context.cancellation);
                started.notify_one();
                std::future::pending::<Result<Value, OperationError>>().await
            }
        },
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut config = config();
    config.allowed_hosts = vec![address.to_string()];
    let app = router(
        config,
        Registry::new([op]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let body = modern(
        "tools/call",
        json!({"name":"testOperation","arguments":{"value":"pending"}}),
    )
    .to_string();
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    write!(
        stream,
        "POST /v1 HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/call\r\nMcp-Name: testOperation\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .unwrap();
    drop(stream);
    let cancellation: CancellationToken = observed.lock().await.clone().unwrap();
    let cancelled = tokio::time::timeout(Duration::from_secs(1), cancellation.cancelled()).await;
    cancellation.cancel();
    server.abort();
    let _ = server.await;
    assert!(cancelled.is_ok(), "native disconnect did not cancel work");
}

#[tokio::test]
async fn should_bound_concurrent_request_admission_before_business_work() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    let op = OperationDefinition::new("testOperation", "Test", policy(), move |_: Arguments, _| {
        let notify = notify.clone();
        async move {
            notify.notify_one();
            std::future::pending::<Result<Value, OperationError>>().await
        }
    })
    .unwrap();
    let mut config = config();
    config.max_concurrent_requests = 1;
    let app = router(
        config,
        Registry::new([op]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let body = modern(
        "tools/call",
        json!({"name":"testOperation","arguments":{"value":"pending"}}),
    );
    let task = tokio::spawn(app.clone().oneshot(request(&body)));
    entered.notified().await;
    assert_eq!(
        StatusCode::SERVICE_UNAVAILABLE,
        app.oneshot(request(&body)).await.unwrap().status()
    );
    task.abort();
}

#[tokio::test]
async fn should_normalize_valid_multiple_accept_values_and_json_media_type_parameters() {
    let mut call = request(&modern("tools/list", json!({})));
    call.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("Application/JSON; charset=utf-8"),
    );
    call.headers_mut()
        .insert("accept", HeaderValue::from_static("application/json"));
    call.headers_mut()
        .append("accept", HeaderValue::from_static("text/event-stream"));
    let response = app(Arc::new(AtomicUsize::new(0)))
        .oneshot(call)
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
}

#[tokio::test]
async fn should_enforce_content_length_and_response_caps_on_admission_error_paths() {
    let mut config = config();
    config.max_response_bytes = 1024;
    let app = router(config, Registry::default(), Arc::new(AnonymousGate)).unwrap();
    let mut call = request(&modern("tools/list", json!({})));
    call.headers_mut()
        .insert("content-length", HeaderValue::from_static("262145"));
    assert_eq!(
        StatusCode::PAYLOAD_TOO_LARGE,
        app.clone().oneshot(call).await.unwrap().status()
    );
    let mut call = request(&modern("tools/list", json!({})));
    call.headers_mut()
        .insert("content-length", HeaderValue::from_static("invalid"));
    assert_eq!(
        StatusCode::BAD_REQUEST,
        app.clone().oneshot(call).await.unwrap().status()
    );
    let mut body = modern("tools/list", json!({}));
    body["id"] = "x".repeat(4096).into();
    let mut call = request(&body);
    call.headers_mut()
        .insert("mcp-method", HeaderValue::from_static("tools/call"));
    let response = app.oneshot(call).await.unwrap();
    assert_eq!(StatusCode::INTERNAL_SERVER_ERROR, response.status());
    assert_eq!("no-store", response.headers()["cache-control"]);
    let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    assert!(bytes.len() < 1024);
    assert!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("transport limit")
    );
}

#[tokio::test]
async fn should_apply_request_deadline_to_authentication_before_dispatch() {
    let cancellation = Arc::new(tokio::sync::Mutex::new(None));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut config = config();
    config.request_timeout = Duration::from_millis(10);
    let app = router(
        config,
        Registry::new([operation(calls.clone())]).unwrap(),
        Arc::new(PendingGate(cancellation.clone())),
    )
    .unwrap();
    let response = app
        .oneshot(request(&modern(
            "tools/call",
            json!({"name":"testOperation","arguments":{"value":"private"}}),
        )))
        .await
        .unwrap();
    assert_eq!(StatusCode::REQUEST_TIMEOUT, response.status());
    assert_eq!(0, calls.load(Ordering::SeqCst));
    let token = cancellation.lock().await.clone().unwrap();
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn should_reject_origin_values_that_a_forgiving_url_parser_would_repair() {
    let mut config = config();
    config.allowed_origins = vec!["https://app.example.test".into()];
    let app = router(config, Registry::default(), Arc::new(AnonymousGate)).unwrap();
    for origin in [
        "https://app.example.test/.",
        "https://app.example.test/%2e",
        r"https:\\app.example.test",
        r"https://app.example.test\",
        "https:///app.example.test",
        "https:app.example.test",
    ] {
        let mut call = request(&modern("tools/list", json!({})));
        call.headers_mut().insert("origin", origin.parse().unwrap());
        assert_eq!(
            StatusCode::FORBIDDEN,
            app.clone().oneshot(call).await.unwrap().status(),
            "{origin}"
        );
    }
}

#[tokio::test]
async fn should_reject_invalid_accept_quality_values_before_business_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(calls.clone());
    for quality in [
        "NaN",
        "bogus",
        "1.5",
        "-1",
        "0e0",
        "0.0001",
        "1.001",
        "0.5;q=1",
        "0.5;q=bogus",
    ] {
        let mut call = request(&modern(
            "tools/call",
            json!({"name":"testOperation","arguments":{"value":"private"}}),
        ));
        call.headers_mut().insert(
            "accept",
            format!("application/json;q={quality},text/event-stream")
                .parse()
                .unwrap(),
        );
        assert_eq!(
            StatusCode::NOT_ACCEPTABLE,
            app.clone().oneshot(call).await.unwrap().status(),
            "{quality}"
        );
    }
    assert_eq!(0, calls.load(Ordering::SeqCst));
    let mut call = request(&modern("tools/list", json!({})));
    call.headers_mut().insert(
        "accept",
        HeaderValue::from_static("application/json;q=0,text/event-stream,*/*;q=1"),
    );
    assert_eq!(
        StatusCode::NOT_ACCEPTABLE,
        app.clone().oneshot(call).await.unwrap().status()
    );
    for accept in [
        "application/json;q=0.5,text/event-stream;Q=1.000",
        "Application/JSON,Text/Event-Stream",
    ] {
        let mut call = request(&modern("tools/list", json!({})));
        call.headers_mut().insert("accept", accept.parse().unwrap());
        assert_eq!(
            StatusCode::OK,
            app.clone().oneshot(call).await.unwrap().status()
        );
    }
}

#[tokio::test]
async fn should_reject_ambiguous_raw_envelopes_before_admission_or_dispatch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let admissions = Arc::new(AtomicUsize::new(0));
    let app = router(
        config(),
        Registry::new([operation(calls.clone())]).unwrap(),
        Arc::new(CountingGate(admissions.clone())),
    )
    .unwrap();
    let call = r#"{"name":"testOperation","arguments":{"value":"okay"}}"#;
    let bodies=[
        format!(r#"{{"jsonrpc":"2.0","jsonrpc":"2.0","id":1,"method":"tools/call","params":{call}}}"#),
        format!(r#"{{"jsonrpc":"2.0","id":1,"id":2,"method":"tools/call","params":{call}}}"#),
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list","method":"tools/call","params":{call}}}"#),
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{}},"params":{call}}}"#),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"otherOperation","name":"testOperation","arguments":{"value":"okay"}}}"#.into(),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"testOperation","arguments":{"value":"bad"},"arguments":{"value":"okay"}}}"#.into(),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"testOperation","arguments":{"value":"okay"},"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-03-26","io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#.into(),
    ];
    for body in bodies {
        let request = Request::builder()
            .method("POST")
            .uri("/v1")
            .header("host", "mcp.example.test")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(body))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(-32600, json_response(response).await["error"]["code"]);
    }
    assert_eq!(0, admissions.load(Ordering::SeqCst));
    assert_eq!(0, calls.load(Ordering::SeqCst));
}
