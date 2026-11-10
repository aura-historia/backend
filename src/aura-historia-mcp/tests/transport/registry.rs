use super::*;

#[tokio::test]
async fn should_return_safe_distinct_operation_errors_without_private_failure_causes() {
    for (error, code, tool_code, message) in [
        (
            OperationError::InvalidInput,
            None,
            Some("INVALID_INPUT"),
            None,
        ),
        (
            OperationError::NotFound("Listing not found: check the productListingId."),
            None,
            Some("NOT_FOUND"),
            Some("Listing not found: check the productListingId."),
        ),
        (
            OperationError::conflict("Listing is already on the watchlist."),
            None,
            Some("CONFLICT"),
            Some("Listing is already on the watchlist."),
        ),
        (
            OperationError::Conflict {
                message: "Stale expectedVersion: re-read the address and retry.",
                source: Some(std::io::Error::other("private token secret").into()),
            },
            None,
            Some("CONFLICT"),
            Some("Stale expectedVersion: re-read the address and retry."),
        ),
        (
            OperationError::Unavailable(std::io::Error::other("private token secret").into()),
            None,
            Some("TEMPORARILY_UNAVAILABLE"),
            None,
        ),
        (
            OperationError::Internal(std::io::Error::other("private token secret").into()),
            Some(-32603),
            None,
            None,
        ),
    ] {
        let error = Arc::new(std::sync::Mutex::new(Some(error)));
        let op = OperationDefinition::new(
            "testOperation",
            "testOperation",
            "Test",
            policy(),
            move |_: Arguments, _| {
                let error = error.lock().unwrap().take().unwrap();
                async { Err::<Value, _>(error) }
            },
        )
        .unwrap();
        let app = router(
            config(),
            Registry::new([op]).unwrap(),
            Arc::new(AnonymousGate),
        )
        .unwrap();
        let body = json_response(
            app.oneshot(request(&modern(
                "tools/call",
                json!({"name":"testOperation","arguments":{"value":"private"}}),
            )))
            .await
            .unwrap(),
        )
        .await;
        assert!(!body.to_string().contains("private"));
        assert!(!body.to_string().contains("secret"));
        if let Some(code) = code {
            assert_eq!(code, body["error"]["code"]);
        }
        if let Some(code) = tool_code {
            assert_eq!(true, body["result"]["isError"]);
            assert_eq!(code, body["result"]["content"][0]["text"]);
            match message {
                Some(message) => {
                    assert_eq!(message, body["result"]["content"][1]["text"]);
                    assert_eq!(2, body["result"]["content"].as_array().unwrap().len());
                }
                None => assert_eq!(1, body["result"]["content"].as_array().unwrap().len()),
            }
        }
    }
}

#[tokio::test]
async fn should_reject_oversized_result_without_silently_truncating_it() {
    let op = OperationDefinition::new(
        "testOperation",
        "testOperation",
        "Test",
        policy(),
        |_: Arguments, _| async { Ok("x".repeat(3 * 1024 * 1024)) },
    )
    .unwrap();
    let app = router(
        config(),
        Registry::new([op]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let response = app
        .oneshot(request(&modern(
            "tools/call",
            json!({"name":"testOperation","arguments":{"value":"small"}}),
        )))
        .await
        .unwrap();
    let body = json_response(response).await;
    assert_eq!(-32603, body["error"]["code"]);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("transport limit")
    );
    assert!(body.get("result").is_none());
}

#[tokio::test]
async fn should_page_tools_in_stable_operation_id_order_and_reject_invalid_cursors() {
    let operations = (0..53).rev().map(|index| {
        let name: &'static str = Box::leak(format!("testOperation{index:02}").into_boxed_str());
        OperationDefinition::new(name, name, "Test", policy(), |_: Arguments, _| async {
            Ok(Output {
                value: "ok".into(),
                principal: "anonymous".into(),
                request_id: "request".into(),
                correlation_id: "request".into(),
            })
        })
        .unwrap()
    });
    let app = router(
        config(),
        Registry::new(operations).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(request(&modern("tools/list", json!({}))))
        .await
        .unwrap();
    let first = json_response(response).await;
    assert_eq!(50, first["result"]["tools"].as_array().unwrap().len());
    assert_eq!("testOperation00", first["result"]["tools"][0]["name"]);
    assert_eq!("testOperation49", first["result"]["nextCursor"]);
    let response = app
        .clone()
        .oneshot(request(&modern(
            "tools/list",
            json!({"cursor":first["result"]["nextCursor"]}),
        )))
        .await
        .unwrap();
    let second = json_response(response).await;
    assert_eq!(3, second["result"]["tools"].as_array().unwrap().len());
    assert_eq!("testOperation50", second["result"]["tools"][0]["name"]);
    assert!(second["result"].get("nextCursor").is_none());
    let response = app
        .oneshot(request(&modern("tools/list", json!({"cursor":"unknown"}))))
        .await
        .unwrap();
    assert_eq!(-32602, json_response(response).await["error"]["code"]);
}

#[tokio::test]
async fn should_preserve_legacy_json_schema_object_requirement_only_where_needed() {
    let ops = [
        OperationDefinition::new(
            "arrayOperation",
            "arrayOperation",
            "Array",
            policy(),
            |_: Arguments, _| async { Ok(vec!["one", "two"]) },
        )
        .unwrap(),
        OperationDefinition::new(
            "nullableOperation",
            "nullableOperation",
            "Nullable",
            policy(),
            |_: Arguments, _| async { Ok(None::<String>) },
        )
        .unwrap(),
    ];
    let app = router(
        config(),
        Registry::new(ops).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(request(&modern("tools/list", json!({}))))
        .await
        .unwrap();
    let tools = json_response(response).await;
    for tool in tools["result"]["tools"].as_array().unwrap() {
        assert_eq!("object", tool["outputSchema"]["type"]);
        assert!(tool["outputSchema"]["properties"].get("result").is_some());
    }
    for (name, expected) in [
        ("arrayOperation", json!({"result":["one","two"]})),
        ("nullableOperation", json!({"result":null})),
    ] {
        let response = app
            .clone()
            .oneshot(request(&modern(
                "tools/call",
                json!({"name":name,"arguments":{"value":"test"}}),
            )))
            .await
            .unwrap();
        assert_eq!(
            expected,
            json_response(response).await["result"]["structuredContent"]
        );
    }
}

#[tokio::test]
async fn should_wrap_mixed_root_results_and_reject_mixed_or_nullable_inputs() {
    #[derive(Serialize, Deserialize, JsonSchema)]
    #[serde(untagged)]
    enum Mixed {
        Object { value: String },
        Text(String),
    }

    assert!(
        OperationDefinition::new(
            "mixedInput",
            "mixedInput",
            "Test",
            policy(),
            |_: Mixed, _| async { Ok(()) }
        )
        .is_err()
    );
    assert!(
        OperationDefinition::new(
            "nullableInput",
            "nullableInput",
            "Test",
            policy(),
            |_: Option<Arguments>, _| async { Ok(()) },
        )
        .is_err()
    );
    let operation = OperationDefinition::new(
        "mixedOutput",
        "mixedOutput",
        "Test",
        policy(),
        |input: Arguments, _| async move {
            Ok(if input.value.is_empty() {
                Mixed::Text("empty".into())
            } else {
                Mixed::Object { value: input.value }
            })
        },
    )
    .unwrap();
    let output_schema = operation.tool().output_schema.as_ref().unwrap().clone();
    let app = router(
        config(),
        Registry::new([operation]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    for (input, expected) in [
        ("", json!({"result":"empty"})),
        ("value", json!({"result":{"value":"value"}})),
    ] {
        let response = app
            .clone()
            .oneshot(request(&modern(
                "tools/call",
                json!({"name":"mixedOutput","arguments":{"value":input}}),
            )))
            .await
            .unwrap();
        let body = json_response(response).await;
        assert_eq!(expected, body["result"]["structuredContent"]);
        assert!(jsonschema::is_valid(&json!(output_schema), &expected));
    }
}

#[tokio::test]
async fn should_preserve_genuine_serialization_failure_as_safe_internal_error() {
    let op = OperationDefinition::new(
        "testOperation",
        "testOperation",
        "Test",
        policy(),
        |_: Arguments, _| async { Ok(BrokenOutput) },
    )
    .unwrap();
    let app = router(
        config(),
        Registry::new([op]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let response = app
        .oneshot(request(&modern(
            "tools/call",
            json!({"name":"testOperation","arguments":{"value":"test"}}),
        )))
        .await
        .unwrap();
    let body = json_response(response).await;
    assert_eq!(-32603, body["error"]["code"]);
    assert_eq!("Operation failed", body["error"]["message"]);
}

#[test]
fn should_require_unique_display_titles_and_publish_them_with_all_four_hints() {
    let define = |name: &'static str, title: &'static str| {
        OperationDefinition::new(name, title, "Test", policy(), |_: Arguments, _| async {
            Ok(())
        })
    };
    let too_long: &'static str = Box::leak("x".repeat(65).into_boxed_str());
    for title in [
        "",
        " Leading space",
        "Trailing space ",
        "Line\nbreak",
        too_long,
    ] {
        assert!(
            matches!(define("testOperation", title), Err(RegistryError::Title)),
            "{title:?}"
        );
    }
    assert!(define("testOperation", &too_long[1..]).is_ok());
    assert!(matches!(
        Registry::new([
            define("firstOperation", "Same title").unwrap(),
            define("secondOperation", "same Title").unwrap(),
        ]),
        Err(RegistryError::DuplicateTitle)
    ));
    let tool =
        serde_json::to_value(define("testOperation", "Test operation").unwrap().tool()).unwrap();
    assert_eq!("testOperation", tool["name"]);
    assert_eq!("Test operation", tool["title"]);
    for hint in [
        "readOnlyHint",
        "destructiveHint",
        "idempotentHint",
        "openWorldHint",
    ] {
        assert!(tool["annotations"][hint].is_boolean(), "{hint}");
    }
}

#[test]
fn should_reject_invalid_registry_and_transport_configuration() {
    let calls = Arc::new(AtomicUsize::new(0));
    assert!(matches!(
        Registry::new([operation(calls.clone()), operation(calls)]),
        Err(RegistryError::Duplicate)
    ));
    assert!(
        OperationDefinition::new(
            "invalid name",
            "invalid name",
            "Test",
            policy(),
            |_: Arguments, _| async { Ok(()) }
        )
        .is_err()
    );
    assert!(
        OperationDefinition::new(
            "testOperation",
            "testOperation",
            "Test",
            policy(),
            |_: String, _| async { Ok(()) }
        )
        .is_err()
    );
    let mut invalid_policy = policy();
    invalid_policy.destructive = true;
    assert!(
        OperationDefinition::new(
            "testOperation",
            "testOperation",
            "Test",
            invalid_policy,
            |_: Arguments, _| async { Ok(()) }
        )
        .is_err()
    );
    let mut invalid = config();
    invalid.allowed_origins = vec!["https://app.example.test:*".into()];
    assert!(router(invalid, Registry::default(), Arc::new(AnonymousGate)).is_err());
    let mut invalid = config();
    invalid.max_response_bytes = 2 * 1024 * 1024 + 1;
    assert!(router(invalid, Registry::default(), Arc::new(AnonymousGate)).is_err());
}
