use super::*;

#[derive(Clone)]
struct Gate;
#[async_trait::async_trait]
impl crate::AdmissionGate for Gate {
    async fn admit(
        &self,
        _: crate::AdmissionRequest<'_>,
    ) -> Result<Principal, crate::AdmissionRejection> {
        Ok(context().operation.principal)
    }
}
async fn raw_http(registry: Registry, name: &str, args: &str) -> Value {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let app = crate::router(
        crate::TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        registry,
        Arc::new(Gate),
    )
    .unwrap();
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{args}}}}}"#
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1")
                .header("host", "mcp.example.test")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(Body::from(request))
                .unwrap(),
        )
        .await
        .unwrap();
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
fn full_input(source: ListingSourceId) -> Value {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../../docs/fixtures/shipping-imports.json"
    ))
    .unwrap();
    let mut body = fixture["providerMembership"].clone();
    let mut terms = fixture["create"]["content"]["terms"].clone();
    terms["collection"]["site"] = json!({"kind":"ASSERTED","geography":null,"position":{"point":{"latitude_degrees":52.5,"longitude_degrees":13.4},"precision":"PREMISES","accuracy_metres":5.0}});
    terms["shipping"]["coverage"]["zones"][0]["services"][0]["rate"]["price"]["amount"]["amount_minor"] =
        json!(u64::MAX);
    body["delivery"]["value"]["selection"] =
        json!({"state":"SET","value":{"kind":"INLINE","terms":terms}});
    body["delivery"]["value"]["shipping"] =
        json!({"state":"SET","value":{"kind":"REPLACE","value":terms["shipping"]}});
    body["delivery"]["value"]["collection"] =
        json!({"state":"SET","value":{"kind":"REPLACE","value":terms["collection"]}});
    body["listingSourceId"] = json!(source);
    body["sourceListingId"] = json!("opaque");
    body
}
#[tokio::test]
async fn should_reject_duplicate_command_fields_through_raw_http_before_any_effect() {
    for name in [CHANGE, SUBMIT] {
        let (registry, services, publisher) = fixture(ChangeOutcome::Changed);
        let body = full_input(ListingSourceId::new()).to_string();
        for (field, duplicate) in [
            (
                r#""expectedVersion":1"#,
                r#""expectedVersion":1,"expectedVersion":2"#,
            ),
            (
                r#""kind":"PROVIDER""#,
                r#""kind":"PROVIDER","kind":"PROVIDER""#,
            ),
            (
                r#""completeness":"PARTIAL""#,
                r#""completeness":"PARTIAL","completeness":"COMPLETE""#,
            ),
            (
                r#""latitude_degrees":52.5"#,
                r#""latitude_degrees":52.5,"latitude_degrees":53.5"#,
            ),
            (
                r#""amount_minor":18446744073709551615"#,
                r#""amount_minor":1,"amount_minor":18446744073709551615"#,
            ),
        ] {
            assert!(body.contains(field), "{field}");
            let args = body.replacen(field, duplicate, 1);
            let response = raw_http(registry.clone(), name, &args).await;
            assert_eq!(
                response["error"]["code"], -32602,
                "{name}: {field}: {response}"
            );
            assert!(services.writes.lock().unwrap().is_empty());
            assert!(publisher.0.lock().unwrap().is_empty());
        }
    }
}
#[tokio::test]
async fn should_reject_private_number_objects_in_nested_delivery_values_before_any_effect() {
    for name in [CHANGE, SUBMIT] {
        let (registry, services, publisher) = fixture(ChangeOutcome::Changed);
        let body = full_input(ListingSourceId::new()).to_string();
        for (field, forged) in [
            (
                r#""latitude_degrees":52.5"#,
                r#""latitude_degrees":{"$serde_json::private::Number":"52.5"}"#,
            ),
            (
                r#""amount_minor":18446744073709551615"#,
                r#""amount_minor":{"$serde_json::private::Number":"18446744073709551615"}"#,
            ),
        ] {
            let args = body.replacen(field, forged, 1);
            let response = raw_http(registry.clone(), name, &args).await;
            assert_eq!(response["error"]["code"], -32602, "{response}");
            assert!(services.writes.lock().unwrap().is_empty());
            assert!(publisher.0.lock().unwrap().is_empty());
        }
    }
}
#[tokio::test]
async fn should_preserve_nested_decimal_sites_and_full_width_money_through_raw_http() {
    for name in [CHANGE, SUBMIT] {
        let (registry, services, publisher) = fixture(ChangeOutcome::Changed);
        let source = ListingSourceId::new();
        let mut body = full_input(source);
        body["delivery"]["value"]["rateInputs"] = json!({"state":"UNCHANGED","value":null});
        body["delivery"]["value"]["collection"] =
            json!({"state":"SET","value":{"kind":"INHERIT","value":null}});
        let response = raw_http(registry, name, &body.to_string()).await;
        assert!(response.get("error").is_none(), "{response}");
        assert_ne!(response["result"]["isError"], true, "{response}");
        body.as_object_mut().unwrap().remove("listingSourceId");
        body.as_object_mut().unwrap().remove("sourceListingId");
        let expected = contract::decode_command(
            body,
            ProductListingKey::new(source, "opaque".try_into().unwrap()),
        )
        .unwrap();
        if name == CHANGE {
            assert_eq!(services.writes.lock().unwrap()[0].1, expected);
            assert!(publisher.0.lock().unwrap().is_empty());
        } else {
            assert_eq!(
                publisher.0.lock().unwrap()[0].intent,
                ProductListingIngestionIntent::ChangeDelivery(expected)
            );
            assert!(services.writes.lock().unwrap().is_empty());
        }
    }
}
