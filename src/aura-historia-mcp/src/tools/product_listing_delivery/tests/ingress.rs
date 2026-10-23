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
#[tokio::test]
async fn should_reject_duplicate_command_fields_through_raw_http_before_any_effect() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    for name in [CHANGE, SUBMIT] {
        let (registry, services, publisher) = fixture(ChangeOutcome::Changed);
        let app = crate::router(
            crate::TransportConfig {
                allowed_hosts: vec!["mcp.example.test".into()],
                ..Default::default()
            },
            registry,
            Arc::new(Gate),
        )
        .unwrap();
        let args = input(ListingSourceId::new(), "opaque")
            .to_string()
            .replacen("{", r#"{"expectedVersion":2,"#, 1);
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
        let response: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(response["error"]["code"], -32602, "{response}");
        assert!(services.writes.lock().unwrap().is_empty());
        assert!(publisher.0.lock().unwrap().is_empty());
    }
}
