use super::*;
use crate::AuraTokenGate;
use application::operation_context::{CredentialCapability, OperationContext, Principal};
use async_trait::async_trait;
use user_core::{
    access_token::{RawAccessToken, Scope},
    user_id::UserId,
};
use user_service::use_cases::{
    AuthenticateAccessTokenError, AuthenticateAccessTokenRequest, AuthenticateAccessTokenResult,
    AuthenticateAccessTokenUseCase,
};

struct ValidToken {
    user_id: UserId,
    scopes: Vec<Scope>,
}
#[async_trait]
impl AuthenticateAccessTokenUseCase for ValidToken {
    async fn execute(
        &self,
        context: &OperationContext,
        _: AuthenticateAccessTokenRequest,
    ) -> Result<AuthenticateAccessTokenResult, AuthenticateAccessTokenError> {
        assert_eq!(Principal::Anonymous, context.principal);
        Ok(AuthenticateAccessTokenResult {
            user_id: self.user_id,
            scopes: self.scopes.iter().copied().collect(),
        })
    }
}

pub(super) fn authenticated_app(
    services: PublicServices,
    user_id: UserId,
    scopes: Vec<Scope>,
) -> Router {
    let config = TransportConfig {
        allowed_hosts: vec!["mcp.example.test".into()],
        ..Default::default()
    };
    router(
        config.clone(),
        registry(services).unwrap(),
        Arc::new(AuraTokenGate::new(
            Arc::new(ValidToken { user_id, scopes }),
            config.resource_policy,
        )),
    )
    .unwrap()
}

fn authenticated_request(name: &str, arguments: Value, token: &str) -> Request<Body> {
    let body = body("tools/call", json!({"name":name,"arguments":arguments}));
    Request::builder()
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
        .unwrap()
}

pub(super) async fn authenticated_call(app: &Router, name: &str, arguments: Value) -> Value {
    let token = String::from(RawAccessToken::new());
    let request = authenticated_request(name, arguments, &token);
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!("no-store", response.headers()["cache-control"]);
    let response: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_ne!(
        Some(true),
        response["result"]["isError"].as_bool(),
        "{name}: {response}"
    );
    assert!(response.get("error").is_none(), "{name}: {response}");
    let output = response["result"]["structuredContent"].clone();
    let text: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(output, text, "{name}");
    output
}

#[tokio::test]
async fn should_forward_exact_delegated_context_to_all_ten_optional_auth_operations() {
    for (scopes, capabilities) in [
        (vec![], vec![]),
        (
            vec![Scope::WatchlistRead],
            vec![CredentialCapability::WatchlistRead],
        ),
        (
            vec![
                Scope::UsersRead,
                Scope::WatchlistRead,
                Scope::SearchFiltersRead,
                Scope::NotificationsRead,
            ],
            vec![
                CredentialCapability::UsersRead,
                CredentialCapability::WatchlistRead,
                CredentialCapability::SearchFiltersRead,
                CredentialCapability::NotificationsRead,
            ],
        ),
    ] {
        let user_id = UserId::new();
        let fixture = Arc::new(Fixture {
            principal: Principal::DelegatedUser {
                user_id,
                capabilities: capabilities.into_iter().collect(),
            },
            ..Default::default()
        });
        let app = authenticated_app(fixture.services(), user_id, scopes);
        for (name, arguments) in inputs() {
            authenticated_call(&app, name, arguments).await;
        }
        assert_eq!(12, fixture.calls.lock().unwrap().len());
    }
}

struct RejectedToken;
#[async_trait]
impl AuthenticateAccessTokenUseCase for RejectedToken {
    async fn execute(
        &self,
        _: &OperationContext,
        _: AuthenticateAccessTokenRequest,
    ) -> Result<AuthenticateAccessTokenResult, AuthenticateAccessTokenError> {
        // Deleted/revoked and unknown credentials share the authoritative NotFound outcome.
        Err(AuthenticateAccessTokenError::NotFound)
    }
}

#[tokio::test]
async fn should_reject_invalid_or_revoked_credentials_before_any_public_service_execution() {
    let fixture = Arc::new(Fixture::default());
    let config = TransportConfig {
        allowed_hosts: vec!["mcp.example.test".into()],
        ..Default::default()
    };
    let app = router(
        config.clone(),
        registry(fixture.services()).unwrap(),
        Arc::new(AuraTokenGate::new(
            Arc::new(RejectedToken),
            config.resource_policy,
        )),
    )
    .unwrap();
    for token in ["invalid".to_owned(), String::from(RawAccessToken::new())] {
        for (name, arguments) in minimal_inputs() {
            let request = authenticated_request(name, arguments, &token);
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(StatusCode::UNAUTHORIZED, response.status(), "{name}");
            assert_eq!("no-store", response.headers()["cache-control"]);
            assert!(
                response.headers()["www-authenticate"]
                    .to_str()
                    .unwrap()
                    .contains("invalid_token")
            );
        }
    }
    assert!(fixture.calls.lock().unwrap().is_empty());
}
