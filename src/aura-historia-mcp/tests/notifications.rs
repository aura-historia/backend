use application::operation_context::{CredentialCapability, Principal};
use aura_historia_mcp::{
    AdmissionGate, AdmissionRejection, AdmissionRequest, AuraTokenGate, Registry, TransportConfig,
    router, tools::notifications,
};
use aura_historia_runtime::Runtime;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use notification_core::notification_id::NotificationId;
use oauth_core::resource::AuraResourcePolicy;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
use user_core::{
    access_token::{AccessTokenId, HashedRawAccessToken, RawAccessToken},
    user_id::UserId,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

fn app(runtime: &Runtime, gate: Arc<dyn AdmissionGate>) -> Router {
    router(
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        Registry::new(notifications::operations(runtime.notifications().clone()).unwrap()).unwrap(),
        gate,
    )
    .unwrap()
}

async fn request(
    app: &Router,
    token: Option<&str>,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/v1")
        .header("host", "mcp.example.test")
        .header("content-type", "application/json")
        .header("accept", "application/json,text/event-stream");
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let body = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    assert_eq!(
        Some("no-store"),
        res.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
    );
    let bytes = to_bytes(res.into_body(), 1_000_000).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

async fn call(app: &Router, token: &str, name: &str, arguments: Value) -> Value {
    let (status, body) = request(
        app,
        Some(token),
        "tools/call",
        json!({"name":name,"arguments":arguments}),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{body}");
    body
}

fn success(response: &Value) -> Value {
    assert!(response.get("error").is_none(), "{response}");
    let result = &response["result"];
    assert_ne!(Some(true), result["isError"].as_bool(), "{result}");
    let value = result["structuredContent"].clone();
    assert_eq!(
        value,
        serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap()
    );
    value
}
fn acknowledged(response: &Value) {
    assert_eq!(json!({"result":null}), success(response));
}
fn tool_error(response: &Value, code: &str) {
    assert_eq!(true, response["result"]["isError"]);
    assert_eq!(code, response["result"]["content"][0]["text"]);
}

async fn user(pool: &sqlx::PgPool) -> UserId {
    let id = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(id.into_uuid())
        .bind(format!("{id}@example.test"))
        .execute(pool)
        .await
        .unwrap();
    id
}
async fn token(pool: &sqlx::PgPool, user: UserId, scopes: Vec<&str>) -> String {
    let raw = RawAccessToken::new();
    let hash: HashedRawAccessToken = raw.clone().into();
    sqlx::query("INSERT INTO access_tokens(access_token_id,user_id,token_short,token_hash,name,scopes,origin) VALUES($1,$2,$3,$4,'Inbox test',$5,'USER')")
        .bind(AccessTokenId::new().into_uuid()).bind(user.into_uuid()).bind(hash.short_token()).bind(hash.long_token_hash()).bind(scopes)
        .execute(pool).await.unwrap();
    raw.into()
}

async fn seed(pool: &sqlx::PgPool, user: UserId, kind: &str, payload: Value) -> NotificationId {
    let id = NotificationId::new();
    let product = kind.starts_with("WATCHLIST") || kind == "SEARCH_FILTER_MATCH";
    sqlx::query("INSERT INTO notifications(notification_id,user_id,kind,origin_event_id,product_listing_id,user_search_filter_id,partnership_application_id,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(id.into_uuid()).bind(user.into_uuid()).bind(kind)
        .bind(product.then(|| UserId::new().into_uuid())).bind(product.then(|| UserId::new().into_uuid()))
        .bind((kind == "SEARCH_FILTER_MATCH").then(|| UserId::new().into_uuid()))
        .bind((!product).then(|| UserId::new().into_uuid())).bind(payload).execute(pool).await.unwrap();
    id
}
fn partnership() -> Value {
    json!({"type":"PARTNERSHIP_APPLICATION","snapshot":{"party_name":"Test Party","listing_source_name":"Test Source","image":null}})
}
fn snapshot() -> Value {
    json!({"listing_source_id":UserId::new().into_uuid().to_string(), "source_listing_id":"source-item", "listing_source_slug_id":"test-source", "product_listing_title_slug_id":"violin-a1b2c3", "listing_source_name":"Snapshot Source", "title":[{"language":"en","title":"Violin"},{"language":"de","title":"Geige"}], "image":"https://example.test/violin.jpg", "url":"https://example.test/item", "view_url":"https://example.test/item?view=1"})
}
fn sensitive_snapshot() -> Value {
    let mut snapshot = snapshot();
    snapshot["content_policy"] = json!({"decision":"REQUIRES_CONSENT","category":"NAZI_GERMANY"});
    snapshot
}
async fn seen(pool: &sqlx::PgPool, id: NotificationId) -> bool {
    sqlx::query_scalar("SELECT seen FROM notifications WHERE notification_id=$1")
        .bind(id.into_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn count(pool: &sqlx::PgPool, user: UserId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id=$1")
        .bind(user.into_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_preserve_complete_payloads_preferences_pagination_scopes_and_own_inbox_mutations() {
    let pool = get_postgres_client().await;
    let owner = user(&pool).await;
    let foreign = user(&pool).await;
    let read = token(&pool, owner, vec!["notifications:read"]).await;
    let write = token(&pool, owner, vec!["notifications:write"]).await;
    let foreign_token = token(
        &pool,
        foreign,
        vec!["notifications:read", "notifications:write"],
    )
    .await;
    let foreign_id = seed(
        &pool,
        foreign,
        "PARTNERSHIP_APPLICATION_APPROVED",
        partnership(),
    )
    .await;
    let own = seed_payloads(&pool, owner).await;
    let runtime = Runtime::from_pool(pool.clone());
    assert!(Arc::ptr_eq(
        &runtime.notifications().list,
        &runtime.notifications().list
    ));
    let app = app(
        &runtime,
        Arc::new(AuraTokenGate::new(
            runtime.access_token_authentication().clone(),
            AuraResourcePolicy::production(),
        )),
    );
    let tools = assert_catalogue(&app).await;
    assert_scope_boundaries(&app, &read, &write).await;
    assert_complete_payloads(&app, &read, foreign_id, &tools).await;
    assert_preferences(&pool, &app, owner, &read).await;
    assert_cursor_and_arguments(&app, &read, foreign).await;
    assert_selected_seen(&pool, &app, &write, own, foreign_id).await;
    assert_all_seen(&pool, &app, owner, &read, &write, foreign_id).await;
    assert_deletion(&pool, &app, owner, foreign, &write, own, &foreign_token).await;
    assert_optional_payloads(&pool, &app, owner, &read, &tools).await;
}

async fn seed_payloads(pool: &sqlx::PgPool, owner: UserId) -> NotificationId {
    let own = seed(
        pool,
        owner,
        "PARTNERSHIP_APPLICATION_APPROVED",
        partnership(),
    )
    .await;
    seed(
        pool,
        owner,
        "PARTNERSHIP_APPLICATION_REJECTED",
        partnership(),
    )
    .await;
    seed(pool, owner, "WATCHLIST_PRICE_CHANGED", json!({"type":"WATCHLIST","snapshot":snapshot(),"change":{"type":"PRICE_CHANGE","old_price":{"type":"MONETARY","currency":"EUR","amount":1000},"new_price":{"type":"MONETARY","currency":"EUR","amount":0}}})).await;
    seed(pool, owner, "WATCHLIST_AVAILABILITY_CHANGED", json!({"type":"WATCHLIST","snapshot":sensitive_snapshot(),"change":{"type":"AVAILABILITY_CHANGE","old_availability":null,"new_availability":"SOLD_OUT"}})).await;
    seed(pool, owner, "SEARCH_FILTER_MATCH", json!({"type":"SEARCH_FILTER","snapshot":snapshot(),"user_search_filter_name":"Saved Violins"})).await;
    own
}

async fn assert_catalogue(app: &Router) -> Vec<Value> {
    let (_, catalogue) = request(app, None, "tools/list", json!({})).await;
    let tools = catalogue["result"]["tools"].as_array().unwrap();
    let manifest: Value =
        serde_json::from_str(include_str!("../authenticated-operations.json")).unwrap();
    assert_eq!(6, tools.len());
    for operation in manifest["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|operation| operation["family"] == "notifications")
    {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == operation["name"])
            .unwrap();
        assert_eq!(operation["name"], operation["operationId"]);
        assert_eq!(operation["access"], tool["_meta"]["aura.historia/access"]);
        assert_eq!(
            operation["requiredScopes"],
            tool["_meta"]["aura.historia/requiredScopes"]
        );
        assert_eq!(operation["annotations"], tool["annotations"]);
    }
    tools.clone()
}

async fn assert_scope_boundaries(app: &Router, read: &str, write: &str) {
    let (status, _) = request(
        app,
        None,
        "tools/call",
        json!({"name":"listNotifications","arguments":{}}),
    )
    .await;
    assert_eq!(StatusCode::UNAUTHORIZED, status);
    for name in [
        "updateNotificationSeen",
        "updateNotificationsSeen",
        "updateAllNotificationsSeen",
        "deleteNotification",
        "deleteNotifications",
    ] {
        let (status, _) = request(
            app,
            Some(read),
            "tools/call",
            json!({"name":name,"arguments":{}}),
        )
        .await;
        assert_eq!(StatusCode::FORBIDDEN, status, "{name}");
    }
    let (status, _) = request(
        app,
        Some(write),
        "tools/call",
        json!({"name":"listNotifications","arguments":{}}),
    )
    .await;
    assert_eq!(StatusCode::FORBIDDEN, status);
}

async fn assert_complete_payloads(
    app: &Router,
    read: &str,
    foreign_id: NotificationId,
    tools: &[Value],
) {
    let output = success(
        &call(
            app,
            read,
            "listNotifications",
            json!({"language":"de-DE","size":100}),
        )
        .await,
    );
    let list_tool = tools
        .iter()
        .find(|t| t["name"] == "listNotifications")
        .unwrap();
    let schema = jsonschema::validator_for(&list_tool["outputSchema"]).unwrap();
    assert!(
        schema.is_valid(&output),
        "{output}: {:?}",
        schema.iter_errors(&output).collect::<Vec<_>>()
    );
    assert_eq!(5, output["size"]);
    assert!(output.get("searchAfter").is_none());
    assert!(output.get("total").is_none());
    assert_eq!(
        BTreeSet::from([
            "WATCHLIST_PRICE_CHANGED",
            "WATCHLIST_AVAILABILITY_CHANGED",
            "SEARCH_FILTER_MATCH",
            "PARTNERSHIP_APPLICATION_APPROVED",
            "PARTNERSHIP_APPLICATION_REJECTED",
        ]),
        output["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["kind"].as_str().unwrap())
            .collect()
    );
    for item in output["items"].as_array().unwrap() {
        assert_payload(item, foreign_id);
    }
}

async fn assert_preferences(pool: &sqlx::PgPool, app: &Router, owner: UserId, read: &str) {
    sqlx::query("UPDATE users SET show_unassessed_or_sensitive_content=true WHERE user_id=$1")
        .bind(owner.into_uuid())
        .execute(pool)
        .await
        .unwrap();
    let visible = success(&call(app, read, "listNotifications", json!({})).await);
    for item in visible["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["payload"].get("productListingId").is_some())
    {
        assert_eq!(
            "https://example.test/violin.jpg",
            item["payload"]["image"]["url"]
        );
    }
}

async fn assert_optional_payloads(
    pool: &sqlx::PgPool,
    app: &Router,
    owner: UserId,
    read: &str,
    tools: &[Value],
) {
    let mut absent = snapshot();
    absent["title"] = Value::Null;
    absent["image"] = Value::Null;
    let price = seed(pool, owner, "WATCHLIST_PRICE_CHANGED", json!({"type":"WATCHLIST","snapshot":absent,"change":{"type":"PRICE_CHANGE","old_price":{"type":"ON_REQUEST"},"new_price":null}})).await;
    let availability = seed(pool, owner, "WATCHLIST_AVAILABILITY_CHANGED", json!({"type":"WATCHLIST","snapshot":absent,"change":{"type":"AVAILABILITY_CHANGE","old_availability":"AVAILABLE","new_availability":null}})).await;
    let mut partnership_payload = partnership();
    partnership_payload["snapshot"]["image"] = json!("https://example.test/party.jpg");
    let partnership = seed(
        pool,
        owner,
        "PARTNERSHIP_APPLICATION_APPROVED",
        partnership_payload,
    )
    .await;
    let output = success(&call(app, read, "listNotifications", json!({})).await);
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "listNotifications")
        .unwrap();
    assert!(
        jsonschema::validator_for(&tool["outputSchema"])
            .unwrap()
            .is_valid(&output)
    );
    let items = output["items"].as_array().unwrap();
    assert_eq!(3, items.len());
    for (id, change) in [
        (
            price,
            json!({"type":"PRICE_CHANGE","oldPrice":{"type":"ON_REQUEST"},"newPrice":null}),
        ),
        (
            availability,
            json!({"type":"AVAILABILITY_CHANGE","oldAvailability":"AVAILABLE","newAvailability":null}),
        ),
    ] {
        let item = items
            .iter()
            .find(|item| item["notificationId"] == id.to_string())
            .unwrap();
        let payload = &item["payload"];
        assert_eq!(change, payload["change"]);
        assert_eq!(Some(&Value::Null), payload.get("title"));
        assert_eq!(Some(&Value::Null), payload.get("image"));
    }
    let item = items
        .iter()
        .find(|item| item["notificationId"] == partnership.to_string())
        .unwrap();
    assert_eq!("https://example.test/party.jpg", item["payload"]["image"]);
}

async fn assert_cursor_and_arguments(app: &Router, read: &str, foreign: UserId) {
    let first = success(&call(app, read, "listNotifications", json!({"size":0})).await);
    assert_eq!(1, first["size"]);
    let next = success(
        &call(
            app,
            read,
            "listNotifications",
            json!({"size":1,"searchAfter":first["searchAfter"].to_string()}),
        )
        .await,
    );
    assert_ne!(
        first["items"][0]["notificationId"],
        next["items"][0]["notificationId"]
    );
    for args in [
        json!({"searchAfter":"[]"}),
        json!({"language":"xx"}),
        json!({"userId":foreign.to_string()}),
        json!({"searchAfter":"[\"bad\",\"bad\"]"}),
    ] {
        assert_eq!(
            -32602,
            call(app, read, "listNotifications", args).await["error"]["code"]
        );
    }
}

async fn assert_selected_seen(
    pool: &sqlx::PgPool,
    app: &Router,
    write: &str,
    own: NotificationId,
    foreign_id: NotificationId,
) {
    for id in [foreign_id, NotificationId::new()] {
        for name in ["updateNotificationSeen", "deleteNotification"] {
            let args = if name == "updateNotificationSeen" {
                json!({"notificationId":id,"seen":true})
            } else {
                json!({"notificationId":id})
            };
            tool_error(&call(app, write, name, args).await, "NOT_FOUND");
        }
    }
    acknowledged(
        &call(
            app,
            write,
            "updateNotificationsSeen",
            json!({"notificationIds":[foreign_id,NotificationId::new()],"seen":true}),
        )
        .await,
    );
    assert!(!seen(pool, own).await && !seen(pool, foreign_id).await);
    acknowledged(
        &call(
            app,
            write,
            "updateNotificationSeen",
            json!({"notificationId":own,"seen":true}),
        )
        .await,
    );
    assert!(seen(pool, own).await);
    acknowledged(
        &call(
            app,
            write,
            "updateNotificationsSeen",
            json!({"notificationIds":[own,own,foreign_id,NotificationId::new()],"seen":false}),
        )
        .await,
    );
    assert!(!seen(pool, own).await && !seen(pool, foreign_id).await);
    tool_error(
        &call(
            app,
            write,
            "updateNotificationsSeen",
            json!({"notificationIds":[],"seen":true}),
        )
        .await,
        "INVALID_INPUT",
    );
    assert_eq!(
        -32602,
        call(
            app,
            write,
            "updateNotificationsSeen",
            json!({"notificationIds":["bad"],"seen":true})
        )
        .await["error"]["code"]
    );
}

async fn assert_all_seen(
    pool: &sqlx::PgPool,
    app: &Router,
    owner: UserId,
    read: &str,
    write: &str,
    foreign_id: NotificationId,
) {
    for _ in 0..100 {
        seed(
            pool,
            owner,
            "PARTNERSHIP_APPLICATION_APPROVED",
            partnership(),
        )
        .await;
    }
    assert_eq!(
        100,
        success(&call(app, read, "listNotifications", json!({"size":u64::MAX})).await)["size"]
    );
    acknowledged(
        &call(
            app,
            write,
            "updateAllNotificationsSeen",
            json!({"seen":true}),
        )
        .await,
    );
    let unseen: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id=$1 AND NOT seen")
            .bind(owner.into_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(0, unseen);
    assert!(!seen(pool, foreign_id).await);
    acknowledged(
        &call(
            app,
            write,
            "updateAllNotificationsSeen",
            json!({"seen":false}),
        )
        .await,
    );
    let seen_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id=$1 AND seen")
            .bind(owner.into_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(0, seen_count);
}

async fn assert_deletion(
    pool: &sqlx::PgPool,
    app: &Router,
    owner: UserId,
    foreign: UserId,
    write: &str,
    own: NotificationId,
    foreign_token: &str,
) {
    acknowledged(
        &call(
            app,
            write,
            "deleteNotification",
            json!({"notificationId":own}),
        )
        .await,
    );
    tool_error(
        &call(
            app,
            write,
            "deleteNotification",
            json!({"notificationId":own}),
        )
        .await,
        "NOT_FOUND",
    );
    for args in [
        json!({"notificationIds":[own]}),
        json!({"body":{}}),
        json!({"userId":foreign}),
    ] {
        assert_eq!(
            -32602,
            call(app, write, "deleteNotifications", args).await["error"]["code"]
        );
    }
    assert_eq!(104, count(pool, owner).await);
    acknowledged(&call(app, write, "deleteNotifications", json!({})).await);
    acknowledged(&call(app, write, "deleteNotifications", json!({})).await);
    assert_eq!(0, count(pool, owner).await);
    assert_eq!(1, count(pool, foreign).await);
    assert_eq!(
        1,
        success(&call(app, foreign_token, "listNotifications", json!({})).await)["size"]
    );
}

fn assert_payload(item: &Value, foreign_id: NotificationId) {
    assert_ne!(foreign_id.to_string(), item["notificationId"]);
    assert!(item["created"].is_string() && item["updated"].is_string());
    let payload = &item["payload"];
    match item["kind"].as_str().unwrap() {
        "WATCHLIST_PRICE_CHANGED" => {
            assert_eq!(
                json!({
                    "type":"PRICE_CHANGE",
                    "oldPrice":{"type":"MONETARY","currency":"EUR","amount":1000},
                    "newPrice":{"type":"MONETARY","currency":"EUR","amount":0}
                }),
                payload["change"]
            );
        }
        "WATCHLIST_AVAILABILITY_CHANGED" => {
            assert!(payload["change"]["oldAvailability"].is_null());
            assert_eq!("SOLD_OUT", payload["change"]["newAvailability"]);
        }
        "SEARCH_FILTER_MATCH" => {
            assert_eq!("Saved Violins", payload["userSearchFilterName"]);
            assert!(payload["userSearchFilterId"].is_string());
        }
        "PARTNERSHIP_APPLICATION_APPROVED" | "PARTNERSHIP_APPLICATION_REJECTED" => {
            let decision = if item["kind"] == "PARTNERSHIP_APPLICATION_APPROVED" {
                "APPROVED"
            } else {
                "REJECTED"
            };
            assert_eq!(decision, payload["decision"]);
            assert!(
                payload["partnershipApplicationId"]
                    .as_str()
                    .unwrap()
                    .starts_with("pa_")
            );
            assert_eq!("Test Party", payload["partyName"]);
            assert_eq!("Test Source", payload["listingSourceName"]);
            assert_eq!(Some(&Value::Null), payload.get("image"));
        }
        kind => panic!("unexpected kind {kind}"),
    }
    if payload.get("productListingId").is_some() {
        for field in [
            "productListingId",
            "listingSourceId",
            "sourceListingId",
            "listingSourceSlugId",
            "productListingTitleSlugId",
            "listingSourceName",
            "url",
            "viewUrl",
        ] {
            assert!(payload[field].is_string(), "{field}: {payload}");
        }
        assert_eq!("source-item", payload["sourceListingId"]);
        assert_eq!("test-source", payload["listingSourceSlugId"]);
        assert_eq!("violin-a1b2c3", payload["productListingTitleSlugId"]);
        assert_eq!("Snapshot Source", payload["listingSourceName"]);
        assert_eq!("https://example.test/item", payload["url"]);
        assert_eq!("https://example.test/item?view=1", payload["viewUrl"]);
        assert_eq!(json!({"text":"Geige","language":"de"}), payload["title"]);
        assert_eq!(json!({"url":null}), payload["image"]);
    }
}

struct FixedGate(UserId);
#[async_trait::async_trait]
impl AdmissionGate for FixedGate {
    async fn admit(&self, _: AdmissionRequest<'_>) -> Result<Principal, AdmissionRejection> {
        Ok(Principal::DelegatedUser {
            user_id: self.0,
            capabilities: BTreeSet::from([CredentialCapability::NotificationsWrite]),
        })
    }
}
#[tokio::test]
async fn should_bound_deletion_failure_without_exposing_storage_details() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://fixture:fixture@127.0.0.1:9/fixture")
        .unwrap();
    pool.close().await;
    let runtime = Runtime::from_pool(pool);
    let app = app(&runtime, Arc::new(FixedGate(UserId::new())));
    for (name, args) in [
        (
            "deleteNotification",
            json!({"notificationId":NotificationId::new()}),
        ),
        ("deleteNotifications", json!({})),
    ] {
        let result = call(&app, "ignored", name, args).await;
        tool_error(&result, "TEMPORARILY_UNAVAILABLE");
        assert!(!result.to_string().contains("pool closed"));
    }
}
