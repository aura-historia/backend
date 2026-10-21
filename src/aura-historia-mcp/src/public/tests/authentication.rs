use super::*;
use crate::AuraTokenGate;
use application::operation_context::OperationContext;
use async_trait::async_trait;
use user_core::{
    access_token::{RawAccessToken, Scope},
    user_id::UserId,
};
use user_service::use_cases::{
    AuthenticateAccessTokenError, AuthenticateAccessTokenRequest, AuthenticateAccessTokenResult,
    AuthenticateAccessTokenUseCase,
};

struct ValidToken;
#[async_trait]
impl AuthenticateAccessTokenUseCase for ValidToken {
    async fn execute(
        &self,
        context: &OperationContext,
        _: AuthenticateAccessTokenRequest,
    ) -> Result<AuthenticateAccessTokenResult, AuthenticateAccessTokenError> {
        assert_eq!(
            application::operation_context::Principal::Anonymous,
            context.principal
        );
        Ok(AuthenticateAccessTokenResult {
            user_id: UserId::new(),
            scopes: [
                Scope::UsersRead,
                Scope::WatchlistRead,
                Scope::NotificationsRead,
            ]
            .into_iter()
            .collect(),
        })
    }
}
#[tokio::test]
async fn should_authenticate_supplied_credentials_without_activating_public_personalization() {
    let fixture = Arc::new(Fixture::default());
    let token = String::from(RawAccessToken::new());
    let config = TransportConfig {
        allowed_hosts: vec!["mcp.example.test".into()],
        ..Default::default()
    };
    let app = router(
        config.clone(),
        registry(fixture.services()).unwrap(),
        Arc::new(AuraTokenGate::new(
            Arc::new(ValidToken),
            config.resource_policy,
        )),
    )
    .unwrap();
    for (name, arguments) in minimal_inputs() {
        let body = body("tools/call", json!({"name":name,"arguments":arguments}));
        let request = Request::builder()
            .method("POST")
            .uri("/v1")
            .header("host", "mcp.example.test")
            .header("content-type", "application/json")
            .header("accept", "application/json,text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/call")
            .header("mcp-name", name)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(StatusCode::OK, response.status());
        let response: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_ne!(
            Some(true),
            response["result"]["isError"].as_bool(),
            "{response}"
        );
        assert!(response.get("error").is_none(), "{response}");
        assert!(!response.to_string().contains("userState"));
    }
    assert_eq!(12, fixture.calls.lock().unwrap().len());
}
