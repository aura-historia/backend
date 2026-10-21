use application::operation_context::{CredentialCapability, Principal};
use aura_historia_mcp::{
    AccessClass, AuraTokenGate, OperationDefinition, OperationPolicy, Registry, TransportConfig,
    router,
};
use aura_historia_runtime::Runtime;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use oauth_core::resource::AuraResourcePolicy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
use user_core::{
    access_token::{AccessTokenId, HashedRawAccessToken, RawAccessToken},
    user_id::UserId,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), application::error::BoxError>;

#[derive(Deserialize, JsonSchema)]
struct Arguments {}

fn app(runtime: &Runtime, calls: Arc<AtomicUsize>, user: UserId) -> Router {
    let policy = OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::UsersRead],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    };
    let operation = OperationDefinition::new(
        "protectedFixture",
        "DB authentication fixture",
        policy,
        move |_: Arguments, context| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert!(matches!(context.operation.principal,
                Principal::DelegatedUser { user_id, .. } if user_id == user));
            async { Ok(json!({"allowed": true})) }
        },
    )
    .unwrap();
    let mut operations = aura_historia_mcp::product_listing_history::operations(
        runtime.public_product_listing_history().clone(),
    )
    .unwrap();
    operations.push(operation);
    router(
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        Registry::new(operations).unwrap(),
        Arc::new(AuraTokenGate::new(
            runtime.access_token_authentication().clone(),
            AuraResourcePolicy::production(),
        )),
    )
    .unwrap()
}

fn request(token: &str) -> Request<Body> {
    request_for(token, "protectedFixture", json!({}))
}

fn history_request(token: &str) -> Request<Body> {
    request_for(
        token,
        "getProductListingHistory",
        json!({
            "productListingId": "pl_01k6k0w000e008000000000001",
        }),
    )
}

fn request_for(token: &str, name: &str, arguments: serde_json::Value) -> Request<Body> {
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    });
    Request::builder()
        .method("POST")
        .uri("/v1")
        .header("host", "mcp.example.test")
        .header("content-type", "application/json")
        .header("accept", "application/json,text/event-stream")
        .header("mcp-session-id", "reused-legacy-session")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn insert_credential(pool: &sqlx::PgPool, user: UserId, id: AccessTokenId) -> String {
    let raw = RawAccessToken::new();
    let hash: HashedRawAccessToken = raw.clone().into();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,'mcp-auth@example.test','FREE','USER')")
        .bind(user.into_uuid()).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO access_tokens(access_token_id,user_id,token_short,token_hash,name,scopes,origin) \
         VALUES($1,$2,$3,$4,'MCP fixture',ARRAY['users:read'],'USER')"
    ).bind(id.into_uuid()).bind(user.into_uuid())
        .bind(hash.short_token()).bind(hash.long_token_hash())
        .execute(pool).await.unwrap();
    raw.into()
}

async fn verify_authoritative_changes() -> TestResult {
    let pool = get_postgres_client().await;
    let user = UserId::new();
    let id = AccessTokenId::new();
    let token = insert_credential(&pool, user, id).await;
    let runtime = Runtime::from_pool(pool.clone());
    assert!(Arc::ptr_eq(
        runtime.access_token_authentication(),
        runtime.access_token_authentication()
    ));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(&runtime, calls.clone(), user);
    assert_eq!(
        StatusCode::OK,
        app.clone().oneshot(request(&token)).await?.status()
    );

    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(user.into_uuid())
        .execute(&pool)
        .await?;
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        app.clone().oneshot(request(&token)).await?.status()
    );
    sqlx::query("UPDATE users SET suspended=false WHERE user_id=$1")
        .bind(user.into_uuid())
        .execute(&pool)
        .await?;

    sqlx::query("UPDATE access_tokens SET scopes=ARRAY[]::text[] WHERE access_token_id=$1")
        .bind(id.into_uuid())
        .execute(&pool)
        .await?;
    assert_eq!(
        StatusCode::FORBIDDEN,
        app.clone().oneshot(request(&token)).await?.status()
    );
    let public_response = app.clone().oneshot(history_request(&token)).await?;
    assert_eq!(StatusCode::OK, public_response.status());
    assert_eq!("no-store", public_response.headers()["cache-control"]);
    let public_result: serde_json::Value = serde_json::from_slice(
        &to_bytes(
            public_response.into_body(),
            aura_historia_mcp::MAX_RESPONSE_BYTES,
        )
        .await?,
    )?;
    assert!(public_result.get("error").is_none(), "{public_result}");
    assert_eq!(true, public_result["result"]["isError"]);
    assert_eq!("NOT_FOUND", public_result["result"]["content"][0]["text"]);

    sqlx::query(
        "UPDATE access_tokens SET scopes=ARRAY['users:read'], \
         expires_at=now()-interval '1 second' WHERE access_token_id=$1",
    )
    .bind(id.into_uuid())
    .execute(&pool)
    .await?;
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        app.clone().oneshot(request(&token)).await?.status()
    );
    sqlx::query("UPDATE access_tokens SET expires_at=NULL WHERE access_token_id=$1")
        .bind(id.into_uuid())
        .execute(&pool)
        .await?;
    assert_eq!(
        StatusCode::OK,
        app.clone().oneshot(request(&token)).await?.status()
    );

    sqlx::query("DELETE FROM access_tokens WHERE access_token_id=$1")
        .bind(id.into_uuid())
        .execute(&pool)
        .await?;
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        app.clone().oneshot(request(&token)).await?.status()
    );
    let public_response = app.clone().oneshot(history_request(&token)).await?;
    assert_eq!(StatusCode::UNAUTHORIZED, public_response.status());
    assert_eq!("no-store", public_response.headers()["cache-control"]);
    pool.close().await;
    let response = app.oneshot(request(&token)).await?;
    assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
    assert!(!response.headers().contains_key("www-authenticate"));
    assert_eq!(2, calls.load(Ordering::SeqCst));
    Ok(())
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_apply_database_scope_changes_expiry_suspension_deletion_and_outage_on_next_request()
{
    let result = verify_authoritative_changes().await;
    assert!(result.is_ok(), "{result:?}");
}
