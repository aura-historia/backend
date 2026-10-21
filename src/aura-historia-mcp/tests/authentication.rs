use application::{
    error::static_error,
    operation_context::{CredentialCapability, OperationContext, Principal},
};
use async_trait::async_trait;
use aura_historia_mcp::{
    AccessClass, AuraTokenGate, OperationDefinition, OperationError, OperationPolicy, Registry,
    TransportConfig, router,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderValue, Request, StatusCode},
};
use oauth_core::resource::AuraResourcePolicy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use user_core::{
    access_token::{AccessTokenId, AccessTokenOrigin, HashedRawAccessToken, RawAccessToken, Scope},
    user_id::UserId,
};
use user_service::{
    ports::{
        AccessTokenAuthentication, AccessTokenAuthenticationReadError,
        AccessTokenAuthenticationReader,
    },
    use_cases::{AuthenticateAccessTokenHandler, AuthenticateAccessTokenUseCase},
};

#[derive(Default)]
struct State {
    tokens: HashMap<String, AccessTokenAuthentication>,
    calls: Vec<OperationContext>,
    unavailable: bool,
    corrupt: bool,
}
#[derive(Clone, Default)]
struct Store(Arc<Mutex<State>>);
#[async_trait]
impl AccessTokenAuthenticationReader for Store {
    async fn find_authentication_by_hashed_token(
        &self,
        token: &HashedRawAccessToken,
    ) -> Result<Option<AccessTokenAuthentication>, AccessTokenAuthenticationReadError> {
        let state = self.0.lock().unwrap();
        if state.unavailable {
            return Err(AccessTokenAuthenticationReadError::TemporarilyUnavailable {
                source: static_error("private store detail"),
            });
        }
        if state.corrupt {
            return Err(AccessTokenAuthenticationReadError::InvalidReadModel {
                source: static_error("private persisted detail"),
            });
        }
        Ok(state.tokens.get(token.long_token_hash()).cloned())
    }
}
struct Authentication {
    store: Store,
    handler: AuthenticateAccessTokenHandler<Store>,
}
#[async_trait]
impl AuthenticateAccessTokenUseCase for Authentication {
    async fn execute(
        &self,
        context: &OperationContext,
        request: user_service::use_cases::AuthenticateAccessTokenRequest,
    ) -> Result<
        user_service::use_cases::AuthenticateAccessTokenResult,
        user_service::use_cases::AuthenticateAccessTokenError,
    > {
        self.store.0.lock().unwrap().calls.push(context.clone());
        self.handler.execute(context, request).await
    }
}
impl Store {
    fn insert(&self, user_id: UserId, scopes: &[Scope]) -> String {
        let raw = RawAccessToken::new();
        let hash: HashedRawAccessToken = raw.clone().into();
        self.0.lock().unwrap().tokens.insert(
            hash.long_token_hash().into(),
            AccessTokenAuthentication {
                access_token_id: AccessTokenId::new(),
                user_id,
                scopes: scopes.iter().copied().collect(),
                origin: AccessTokenOrigin::User,
                expires: None,
            },
        );
        raw.into()
    }
    fn authentication(&self) -> Arc<dyn AuthenticateAccessTokenUseCase> {
        Arc::new(Authentication {
            store: self.clone(),
            handler: AuthenticateAccessTokenHandler::new(self.clone()),
        })
    }
    fn calls(&self) -> usize {
        self.0.lock().unwrap().calls.len()
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Arguments {}
fn policy(access: AccessClass, scopes: Vec<CredentialCapability>) -> OperationPolicy {
    OperationPolicy {
        access,
        required_scopes: scopes,
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}
fn config() -> TransportConfig {
    TransportConfig {
        allowed_hosts: vec!["mcp.example.test".into()],
        ..Default::default()
    }
}
fn registry(calls: &Arc<AtomicUsize>, observed: &Arc<Mutex<Vec<OperationContext>>>) -> Registry {
    Registry::new(
        [
            ("publicFixture", AccessClass::Public, vec![]),
            (
                "protectedFixture",
                AccessClass::Protected,
                vec![
                    CredentialCapability::UsersWrite,
                    CredentialCapability::UsersRead,
                ],
            ),
            (
                "serviceDeniedFixture",
                AccessClass::Protected,
                vec![CredentialCapability::UsersRead],
            ),
        ]
        .into_iter()
        .map(|(name, access, scopes)| {
            let calls = calls.clone();
            let observed = observed.clone();
            OperationDefinition::new(
                name,
                "Authentication fixture",
                policy(access, scopes),
                move |_: Arguments, context| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    observed.lock().unwrap().push(context.operation.clone());
                    async move {
                        if name == "serviceDeniedFixture" {
                            Err(OperationError::Forbidden)
                        } else {
                            Ok(json!({"called":true}))
                        }
                    }
                },
            )
            .unwrap()
        }),
    )
    .unwrap()
}
struct Fixture {
    store: Store,
    calls: Arc<AtomicUsize>,
    observed: Arc<Mutex<Vec<OperationContext>>>,
    app: Router,
}
impl Fixture {
    fn new() -> Self {
        let store = Store::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(Mutex::new(vec![]));
        let app = router(
            config(),
            registry(&calls, &observed),
            Arc::new(AuraTokenGate::new(
                store.authentication(),
                AuraResourcePolicy::production(),
            )),
        )
        .unwrap();
        Self {
            store,
            calls,
            observed,
            app,
        }
    }
    async fn call(&self, name: &str, authorization: Option<&str>) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(request(
                "tools/call",
                json!({"name":name,"arguments":{}}),
                authorization,
            ))
            .await
            .unwrap()
    }
}
fn request(method: &str, params: Value, authorization: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/v1")
        .header("host", "mcp.example.test")
        .header("content-type", "application/json")
        .header("accept", "application/json,text/event-stream")
        .header("x-correlation-id", "auth-fixture");
    if let Some(value) = authorization {
        request = request.header("authorization", value);
    }
    request
        .body(Body::from(
            json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
        ))
        .unwrap()
}
async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}
async fn assert_rejection(
    response: axum::response::Response,
    status: StatusCode,
    error: Option<&str>,
    scopes: bool,
) {
    assert_eq!(status, response.status());
    assert_eq!("no-store", response.headers()["cache-control"]);
    let challenge = response.headers()["www-authenticate"].to_str().unwrap();
    assert!(challenge.contains("resource_metadata=\"https://mcp.aura-historia.com/.well-known/oauth-protected-resource/v1\""));
    if let Some(error) = error {
        assert!(challenge.contains(&format!("error=\"{error}\"")));
    } else {
        assert!(!challenge.contains("error="));
    }
    if scopes {
        assert!(challenge.contains("scope=\"users:read users:write\""));
    }
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn should_keep_discovery_and_public_calls_anonymous_without_credentials() {
    let fixture = Fixture::new();
    let response = fixture
        .app
        .clone()
        .oneshot(request("tools/list", json!({}), None))
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!(
        3,
        json_body(response).await["result"]["tools"]
            .as_array()
            .unwrap()
            .len()
    );
    assert_eq!(
        StatusCode::OK,
        fixture.call("publicFixture", None).await.status()
    );
    assert_eq!(
        Principal::Anonymous,
        fixture.observed.lock().unwrap()[0].principal
    );
    assert_eq!(0, fixture.store.calls());
    assert_rejection(
        fixture.call("protectedFixture", None).await,
        StatusCode::UNAUTHORIZED,
        None,
        true,
    )
    .await;
    assert_eq!(1, fixture.calls.load(Ordering::SeqCst));
}
#[tokio::test]
async fn should_reject_every_supplied_invalid_credential_before_public_or_protected_execution() {
    let fixture = Fixture::new();
    let unknown = String::from(RawAccessToken::new());
    for authorization in [
        "Basic private-secret".to_owned(),
        "Bearer ".to_owned(),
        "Bearer private-secret".to_owned(),
        "Bearer eyJhbGciOiJSUzI1NiJ9.cognito.signature".to_owned(),
        format!("Bearer {unknown}"),
        format!("Bearer {unknown},Bearer {unknown}"),
        format!("Bearer  {unknown}"),
    ] {
        for name in ["publicFixture", "protectedFixture"] {
            assert_rejection(
                fixture.call(name, Some(&authorization)).await,
                StatusCode::UNAUTHORIZED,
                Some("invalid_token"),
                name == "protectedFixture",
            )
            .await;
        }
    }
    assert_eq!(0, fixture.calls.load(Ordering::SeqCst));
}
#[tokio::test]
async fn should_reject_supplied_invalid_credentials_for_every_protocol_method() {
    let fixture = Fixture::new();
    for method in [
        "initialize",
        "server/discover",
        "tools/list",
        "ping",
        "notifications/initialized",
        "unknown/method",
    ] {
        let response = fixture
            .app
            .clone()
            .oneshot(request(method, json!({}), Some("Bearer private-sentinel")))
            .await
            .unwrap();
        assert_rejection(
            response,
            StatusCode::UNAUTHORIZED,
            Some("invalid_token"),
            false,
        )
        .await;
    }
    assert_eq!(0, fixture.store.calls());
    assert_eq!(0, fixture.calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_preserve_exact_delegated_identity_and_all_required_scope_challenges() {
    let fixture = Fixture::new();
    let user = UserId::new();
    let token = fixture.store.insert(user, &[Scope::UsersRead]);
    let auth = format!("Bearer {token}");
    assert_rejection(
        fixture.call("protectedFixture", Some(&auth)).await,
        StatusCode::FORBIDDEN,
        Some("insufficient_scope"),
        true,
    )
    .await;
    assert_eq!(0, fixture.calls.load(Ordering::SeqCst));
    assert_eq!(
        StatusCode::OK,
        fixture.call("publicFixture", Some(&auth)).await.status()
    );
    let observed = fixture.observed.lock().unwrap()[0].clone();
    assert_eq!(
        Principal::DelegatedUser {
            user_id: user,
            capabilities: BTreeSet::from([CredentialCapability::UsersRead])
        },
        observed.principal
    );
    let full = fixture
        .store
        .insert(user, &[Scope::UsersRead, Scope::UsersWrite]);
    assert_eq!(
        StatusCode::OK,
        fixture
            .call("protectedFixture", Some(&format!("bearer {full}")))
            .await
            .status()
    );
    let authentication = fixture.store.0.lock().unwrap().calls.clone();
    assert!(
        authentication
            .iter()
            .all(|context| context.principal == Principal::Anonymous
                && context.correlation_id.as_str() == "auth-fixture")
    );
    assert_ne!(authentication[0].request_id, authentication[1].request_id);
}
#[tokio::test]
async fn should_leave_role_and_ownership_denials_with_the_service_after_scope_admission() {
    let fixture = Fixture::new();
    let token = fixture.store.insert(UserId::new(), &[Scope::UsersRead]);
    let response = fixture
        .call("serviceDeniedFixture", Some(&format!("Bearer {token}")))
        .await;
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!(true, json_body(response).await["result"]["isError"]);
    assert_eq!(1, fixture.calls.load(Ordering::SeqCst));
}
#[tokio::test]
async fn should_revalidate_scopes_expiry_and_revocation_on_every_legacy_session_request() {
    let fixture = Fixture::new();
    let token = fixture
        .store
        .insert(UserId::new(), &[Scope::UsersRead, Scope::UsersWrite]);
    let auth = format!("Bearer {token}");
    let call = || {
        let mut request = request(
            "tools/call",
            json!({"name":"protectedFixture","arguments":{}}),
            Some(&auth),
        );
        request
            .headers_mut()
            .insert("mcp-session-id", HeaderValue::from_static("reused-session"));
        request
    };
    assert_eq!(
        StatusCode::OK,
        fixture.app.clone().oneshot(call()).await.unwrap().status()
    );
    fixture
        .store
        .0
        .lock()
        .unwrap()
        .tokens
        .values_mut()
        .next()
        .unwrap()
        .scopes = HashSet::new();
    assert_rejection(
        fixture.app.clone().oneshot(call()).await.unwrap(),
        StatusCode::FORBIDDEN,
        Some("insufficient_scope"),
        true,
    )
    .await;
    fixture
        .store
        .0
        .lock()
        .unwrap()
        .tokens
        .values_mut()
        .next()
        .unwrap()
        .expires = Some(OffsetDateTime::now_utc() - Duration::seconds(1));
    assert_rejection(
        fixture.app.clone().oneshot(call()).await.unwrap(),
        StatusCode::UNAUTHORIZED,
        Some("invalid_token"),
        true,
    )
    .await;
    fixture.store.0.lock().unwrap().tokens.clear();
    assert_rejection(
        fixture.app.clone().oneshot(call()).await.unwrap(),
        StatusCode::UNAUTHORIZED,
        Some("invalid_token"),
        true,
    )
    .await;
    assert_eq!(4, fixture.store.calls());
    assert_eq!(1, fixture.calls.load(Ordering::SeqCst));
}
#[tokio::test]
async fn should_keep_token_store_outages_distinct_from_invalid_credentials_without_execution() {
    let fixture = Fixture::new();
    let token = fixture.store.insert(UserId::new(), &[Scope::UsersRead]);
    let auth = format!("Bearer {token}");
    fixture.store.0.lock().unwrap().unavailable = true;
    let response = fixture.call("publicFixture", Some(&auth)).await;
    assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
    assert_eq!("1", response.headers()["retry-after"]);
    assert!(!response.headers().contains_key("www-authenticate"));
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
    fixture.store.0.lock().unwrap().unavailable = false;
    fixture.store.0.lock().unwrap().corrupt = true;
    let response = fixture.call("protectedFixture", Some(&auth)).await;
    assert_eq!(StatusCode::INTERNAL_SERVER_ERROR, response.status());
    assert!(!response.headers().contains_key("www-authenticate"));
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(0, fixture.calls.load(Ordering::SeqCst));
}
#[tokio::test]
async fn should_reject_ambiguous_headers_and_dispatch_identity_before_authentication() {
    let fixture = Fixture::new();
    let token = fixture
        .store
        .insert(UserId::new(), &[Scope::UsersRead, Scope::UsersWrite]);
    let auth = format!("Bearer {token}");
    for (name, value) in [("mcp-method", "tools/list"), ("mcp-name", "publicFixture")] {
        let mut call = request(
            "tools/call",
            json!({"name":"protectedFixture","arguments":{}}),
            Some(&auth),
        );
        call.headers_mut().insert(name, value.parse().unwrap());
        assert_eq!(
            StatusCode::BAD_REQUEST,
            fixture.app.clone().oneshot(call).await.unwrap().status()
        );
    }
    let mut call = request("tools/list", json!({}), Some(&auth));
    call.headers_mut()
        .append("authorization", auth.parse().unwrap());
    assert_eq!(
        StatusCode::BAD_REQUEST,
        fixture.app.clone().oneshot(call).await.unwrap().status()
    );
    assert_eq!(0, fixture.calls.load(Ordering::SeqCst));
    assert_eq!(0, fixture.store.calls());
}
#[tokio::test]
async fn should_challenge_protected_calls_before_sdk_argument_validation_and_wrapping() {
    let fixture = Fixture::new();
    let call = request(
        "tools/call",
        json!({"name":"protectedFixture","arguments":"private-body"}),
        None,
    );
    assert_rejection(
        fixture.app.clone().oneshot(call).await.unwrap(),
        StatusCode::UNAUTHORIZED,
        None,
        true,
    )
    .await;
    assert_eq!(0, fixture.calls.load(Ordering::SeqCst));
}
#[tokio::test]
async fn should_keep_concurrent_users_isolated_without_reusing_token_authority() {
    let fixture = Fixture::new();
    let first = UserId::new();
    let second = UserId::new();
    let a = fixture.store.insert(first, &[]);
    let b = fixture.store.insert(second, &[Scope::UsersRead]);
    let a = format!("Bearer {a}");
    let b = format!("Bearer {b}");
    let (left, right) = tokio::join!(
        fixture.call("publicFixture", Some(&a)),
        fixture.call("publicFixture", Some(&b))
    );
    assert_eq!(StatusCode::OK, left.status());
    assert_eq!(StatusCode::OK, right.status());
    let contexts = fixture.observed.lock().unwrap();
    assert_eq!(
        Principal::DelegatedUser {
            user_id: first,
            capabilities: BTreeSet::new()
        },
        contexts[0].principal
    );
    assert_eq!(
        Principal::DelegatedUser {
            user_id: second,
            capabilities: BTreeSet::from([CredentialCapability::UsersRead])
        },
        contexts[1].principal
    );
}
#[tokio::test]
async fn should_publish_identical_credential_free_metadata_for_both_paths_in_each_environment() {
    for policy in [
        AuraResourcePolicy::production(),
        AuraResourcePolicy::stage(),
    ] {
        let app = router(
            TransportConfig {
                resource_policy: policy,
                ..config()
            },
            Registry::default(),
            Arc::new(AuraTokenGate::new(
                Store::default().authentication(),
                policy,
            )),
        )
        .unwrap();
        for path in [
            "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-protected-resource/v1",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header("host", "mcp.example.test")
                        .header("authorization", "Bearer intentionally-invalid")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(StatusCode::OK, response.status());
            assert_eq!("no-store", response.headers()["cache-control"]);
            assert_eq!(
                json!({"resource":policy.mcp_resource(),"authorization_servers":[policy.issuer()],"scopes_supported":["users:read"],"bearer_methods_supported":["header"]}),
                json_body(response).await
            );
            for (header, value) in [
                ("host", "attacker.test"),
                ("origin", "https://attacker.test"),
            ] {
                let mut request = Request::builder()
                    .uri(path)
                    .header("host", "mcp.example.test")
                    .body(Body::empty())
                    .unwrap();
                request.headers_mut().insert(header, value.parse().unwrap());
                assert_eq!(
                    StatusCode::FORBIDDEN,
                    app.clone().oneshot(request).await.unwrap().status()
                );
            }
        }
    }
}

#[test]
fn should_require_an_explicit_supported_issuer_in_native_configuration() {
    for issuer in [
        None,
        Some("https://api.aura-historia.com"),
        Some("https://api.stage.aura-historia.com"),
        Some("https://api.aura-historia.com/"),
        Some("https://foreign.example"),
    ] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "issuer_configuration_probe",
                "--ignored",
                "--nocapture",
            ])
            .env_clear();
        if let Some(issuer) = issuer {
            command.env("OAUTH_ISSUER_URL", issuer);
        }
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
#[test]
#[ignore = "invoked by isolated configuration test"]
fn issuer_configuration_probe() {
    let issuer = std::env::var("OAUTH_ISSUER_URL").ok();
    let result = TransportConfig::from_env();
    match issuer.as_deref() {
        Some("https://api.aura-historia.com" | "https://api.stage.aura-historia.com") => {
            assert_eq!(
                issuer.as_deref(),
                Some(result.unwrap().resource_policy.issuer())
            )
        }
        _ => assert!(matches!(
            result,
            Err(aura_historia_mcp::ConfigError::Issuer)
        )),
    }
}

#[tokio::test]
async fn should_preserve_authentication_challenges_through_the_actual_lambda_adapter() {
    let fixture = Fixture::new();
    let request = request(
        "tools/call",
        json!({"name":"protectedFixture","arguments":{}}),
        None,
    );
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 1024).await.unwrap();
    let request =
        lambda_http::Request::from_parts(parts, lambda_http::Body::Binary(bytes.to_vec()));
    let response =
        aura_historia_mcp::lambda::handle_http_api_v2_request(fixture.app.clone(), request)
            .await
            .unwrap();
    assert_rejection(response, StatusCode::UNAUTHORIZED, None, true).await;
    assert_eq!(0, fixture.calls.load(Ordering::SeqCst));
}
