//! Local PostgreSQL acceptance of the production watchlist provider, service policies and HTTP gate.
use application::operation_context::{
    CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
};
use aura_historia_mcp::{
    AuraTokenGate, Registry, TransportConfig, router, tools::watchlist::operations,
};
use aura_historia_runtime::Runtime;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use listing_source_core::ListingSourceId;
use oauth_core::resource::AuraResourcePolicy;
use party_core::party_id::PartyId;
use product_listing_core::product_listing_id::ProductListingId;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{collections::BTreeSet, sync::Arc};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
use user_core::{
    access_token::{AccessTokenId, HashedRawAccessToken, RawAccessToken},
    user_id::UserId,
};
use watchlist_service::use_cases::{
    ListWatchlistRequest, UnwatchProductListingCommand, UnwatchProductListingError,
    UpdateWatchlistProductListingCommand, UpdateWatchlistProductListingError,
    WatchProductListingCommand, WatchProductListingError,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult<T = ()> = Result<T, application::error::BoxError>;

fn app(runtime: &Runtime) -> Router {
    router(
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        Registry::new(operations(runtime.watchlist().clone()).unwrap()).unwrap(),
        Arc::new(AuraTokenGate::new(
            runtime.access_token_authentication().clone(),
            AuraResourcePolicy::production(),
        )),
    )
    .unwrap()
}
async fn call(
    app: &Router,
    token: &str,
    name: &str,
    args: Value,
) -> TestResult<(StatusCode, Value)> {
    let request = Request::post("/v1")
        .header("host", "mcp.example.test")
        .header("content-type", "application/json")
        .header("accept", "application/json,text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/call")
        .header("mcp-name", name)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(json!({
            "jsonrpc":"2.0", "id":1, "method":"tools/call",
            "params":{
                "name":name, "arguments":args,
                "_meta":{
                    "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                    "io.modelcontextprotocol/clientInfo":{"name":"watchlist-fixture","version":"1"},
                    "io.modelcontextprotocol/clientCapabilities":{}
                }
            }
        }).to_string()))?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024).await?;
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    };
    Ok((status, value))
}
fn result(body: &Value) -> &Value {
    &body["result"]["structuredContent"]
}
fn assert_error(body: &Value, code: &str) {
    assert_eq!(true, body["result"]["isError"]);
    assert_eq!(code, body["result"]["content"][0]["text"]);
}
async fn user(pool: &PgPool) -> TestResult<UserId> {
    let id = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(id.as_uuid())
        .bind(format!("{id}@example.test"))
        .execute(pool)
        .await?;
    Ok(id)
}
async fn token(pool: &PgPool, user: UserId, scopes: &[&str]) -> TestResult<String> {
    let raw = RawAccessToken::new();
    let hash: HashedRawAccessToken = raw.clone().into();
    sqlx::query("INSERT INTO access_tokens(access_token_id,user_id,token_short,token_hash,name,scopes,origin) VALUES($1,$2,$3,$4,'Watchlist fixture',$5,'USER')")
        .bind(AccessTokenId::new().as_uuid()).bind(user.as_uuid()).bind(hash.short_token()).bind(hash.long_token_hash()).bind(scopes).execute(pool).await?;
    Ok(raw.into())
}
async fn catalog(pool: &PgPool) -> TestResult<Vec<ProductListingId>> {
    let party = PartyId::new();
    let source = ListingSourceId::new();
    let fx = FxRateId::new();
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,'watchlist-party','Watchlist party')").bind(party.as_uuid()).execute(pool).await?;
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id,url) VALUES($1,'watchlist-source','Watchlist source',$2,'https://shop.example.test')").bind(source.as_uuid()).bind(party.as_uuid()).execute(pool).await?;
    sqlx::query("INSERT INTO fx_rates(fx_rate_id,captured_at,source,source_event_id) VALUES($1,now(),'fxratesapi','watchlist-fixture')").bind(fx.as_uuid()).execute(pool).await?;
    for currency in [
        "EUR", "GBP", "USD", "AUD", "CAD", "NZD", "CNY", "BRL", "PLN", "TRY", "JPY", "CZK", "RUB",
        "AED", "SAR", "HKD", "SGD", "CHF", "ZAR", "SEK", "DKK", "NOK", "KRW", "INR", "TWD", "HUF",
        "RON", "MXN", "THB", "ARS",
    ] {
        sqlx::query(
            "INSERT INTO fx_rate_quotes(fx_rate_id,currency,units_per_eur) VALUES($1,$2,$3)",
        )
        .bind(fx.as_uuid())
        .bind(currency)
        .bind(if currency == "EUR" {
            1_000_000_i64
        } else {
            1_250_000_i64
        })
        .execute(pool)
        .await?;
    }
    let mut products = vec![];
    for index in 0..23 {
        let product = ProductListingId::new();
        let event = EventId::new();
        let mut tx = pool.begin().await?;
        sqlx::query("INSERT INTO product_listings(product_listing_id,product_listing_title_slug_id,current_event_id,content_source_event_id,embedding_source_event_id,listing_source_id,source_listing_id,availability,lifecycle,url,title_text,title_language,price_kind,price_amount,price_currency,product_images) VALUES($1,$2,$3,$3,$3,$4,$5,'AVAILABLE','ACTIVE','https://shop.example.test/item','Watchlist cabinet','en','MONETARY',10000,'EUR',$6)")
            .bind(product.as_uuid()).bind(format!("watchlist-cabinet-{index:06x}")).bind(event.as_uuid()).bind(source.as_uuid()).bind(format!("item-{index}")).bind(json!([{"url":"https://images.example.test/cabinet.jpg"}])).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time) VALUES($1,$2,'PRODUCT_LISTING_DISCOVERED','DOMAIN',1,'{}',now())").bind(event.as_uuid()).bind(product.as_uuid()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO product_listing_translations(product_listing_id,source_event_id,language,title) VALUES($1,$2,'de','Watchlist-Schrank')").bind(product.as_uuid()).bind(event.as_uuid()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO product_listing_content_assessments(product_listing_id,source_event_id,decision,category) VALUES($1,$2,'ALLOWED',NULL)").bind(product.as_uuid()).bind(event.as_uuid()).execute(&mut *tx).await?;
        tx.commit().await?;
        products.push(product);
    }
    Ok(products)
}
struct Fixture {
    runtime: Runtime,
    app: Router,
    first: UserId,
    second: UserId,
    full: String,
    read: String,
    write: String,
    other: String,
    products: Vec<ProductListingId>,
}
impl Fixture {
    async fn new() -> TestResult<Self> {
        let pool = get_postgres_client().await;
        let first = user(&pool).await?;
        let second = user(&pool).await?;
        let full = token(&pool, first, &["watchlist:read", "watchlist:write"]).await?;
        let read = token(&pool, first, &["watchlist:read"]).await?;
        let write = token(&pool, first, &["watchlist:write"]).await?;
        let other = token(&pool, second, &["watchlist:read", "watchlist:write"]).await?;
        let products = catalog(&pool).await?;
        let runtime = Runtime::from_pool(pool);
        assert!(Arc::ptr_eq(
            &runtime.watchlist().list,
            &runtime.watchlist().list
        ));
        let app = app(&runtime);
        Ok(Self {
            runtime,
            app,
            first,
            second,
            full,
            read,
            write,
            other,
            products,
        })
    }
    async fn success(&self, token: &str, name: &str, args: Value) -> TestResult<Value> {
        let (status, body) = call(&self.app, token, name, args).await?;
        assert_eq!(StatusCode::OK, status, "{body}");
        assert_ne!(true, body["result"]["isError"], "{body}");
        assert!(body.get("error").is_none(), "{body}");
        Ok(result(&body).clone())
    }
    async fn entry_snapshot(&self, product: ProductListingId) -> TestResult<Value> {
        Ok(sqlx::query_scalar("SELECT to_jsonb(entry) FROM product_listing_watchlist entry WHERE user_id=$1 AND product_listing_id=$2")
            .bind(self.first.as_uuid()).bind(product.as_uuid()).fetch_one(self.runtime.postgres_pool()).await?)
    }
    async fn error(&self, name: &str, args: Value, code: &str) -> TestResult {
        let (status, body) = call(&self.app, &self.full, name, args).await?;
        assert_eq!(StatusCode::OK, status, "{body}");
        assert_error(&body, code);
        Ok(())
    }
}

async fn verify_add_and_read_scope(fixture: &Fixture) -> TestResult {
    let product = fixture.products[0];
    let entry = fixture
        .success(
            &fixture.full,
            "addWatchlistProduct",
            json!({"productListingId":product}),
        )
        .await?;
    assert_eq!(
        json!({"userId":fixture.first,"productListingId":product,"notifications":true,"state":"ACTIVE"}),
        entry
    );
    let snapshot = fixture.entry_snapshot(product).await?;
    fixture
        .error(
            "addWatchlistProduct",
            json!({"productListingId":product}),
            "CONFLICT",
        )
        .await?;
    assert_eq!(snapshot, fixture.entry_snapshot(product).await?);
    for name in [
        "addWatchlistProduct",
        "patchWatchlistProduct",
        "deleteWatchlistProduct",
    ] {
        let (status, _) = call(
            &fixture.app,
            &fixture.read,
            name,
            json!({"productListingId":product,"notifications":false}),
        )
        .await?;
        assert_eq!(StatusCode::FORBIDDEN, status);
    }
    Ok(())
}

async fn verify_listing_pages(fixture: &Fixture) -> TestResult {
    fixture
        .success(
            &fixture.full,
            "addWatchlistProduct",
            json!({"productListingId":fixture.products[1],"notifications":false}),
        )
        .await?;
    fixture
        .success(
            &fixture.other,
            "addWatchlistProduct",
            json!({"productListingId":fixture.products[2]}),
        )
        .await?;
    let page = fixture
        .success(
            &fixture.read,
            "getWatchlistProductListings",
            json!({"language":"de","currency":"USD","size":1}),
        )
        .await?;
    assert_eq!(1, page["size"]);
    assert_eq!(
        "Watchlist-Schrank",
        page["items"][0]["item"]["title"]["text"]
    );
    assert_eq!(
        12500,
        page["items"][0]["item"]["pricing"]["display"]["price"]["amount"]
    );
    assert!(page["items"][0]["userState"].get("notification").is_none());
    assert!(page["items"][0]["userState"].get("searchFilter").is_none());
    let next = fixture.success(&fixture.read, "getWatchlistProductListings", json!({"language":"de","currency":"USD","size":1,"searchAfter":page["searchAfter"].to_string()})).await?;
    assert_eq!(1, next["size"]);
    assert_ne!(
        page["items"][0]["item"]["productListingId"],
        next["items"][0]["item"]["productListingId"]
    );
    for row in [&page["items"][0], &next["items"][0]] {
        assert_ne!(json!(fixture.products[2]), row["item"]["productListingId"]);
    }
    for name in ["patchWatchlistProduct", "deleteWatchlistProduct"] {
        fixture
            .error(
                name,
                json!({"productListingId":fixture.products[2]}),
                "NOT_FOUND",
            )
            .await?;
    }
    Ok(())
}

async fn verify_service_ownership(fixture: &Fixture) {
    let context = OperationContext {
        principal: Principal::DelegatedUser {
            user_id: fixture.first,
            capabilities: BTreeSet::from([
                CredentialCapability::WatchlistRead,
                CredentialCapability::WatchlistWrite,
            ]),
        },
        request_id: RequestId::new("ownership"),
        correlation_id: CorrelationId::new("ownership"),
    };
    let user_id = fixture.second;
    let product_listing_id = fixture.products[2];
    let services = fixture.runtime.watchlist();
    let language = localization::Language::En;
    let currency = money::Currency::Eur;
    assert!(matches!(
        services
            .add
            .execute(
                &context,
                WatchProductListingCommand {
                    user_id,
                    product_listing_id,
                    notifications: true
                }
            )
            .await,
        Err(WatchProductListingError::Forbidden)
    ));
    assert!(matches!(
        services
            .remove
            .execute(
                &context,
                UnwatchProductListingCommand {
                    user_id,
                    product_listing_id
                }
            )
            .await,
        Err(UnwatchProductListingError::Forbidden)
    ));
    assert!(matches!(
        services
            .update
            .execute(
                &context,
                UpdateWatchlistProductListingCommand {
                    user_id,
                    product_listing_id,
                    notifications: None,
                    state: None,
                    language,
                    currency
                }
            )
            .await,
        Err(UpdateWatchlistProductListingError::Forbidden)
    ));
    assert!(matches!(
        services
            .list
            .execute(
                &context,
                ListWatchlistRequest {
                    user_id,
                    language,
                    currency,
                    cursor: Default::default()
                }
            )
            .await,
        Err(watchlist_service::use_cases::ListWatchlistError::Forbidden)
    ));
}

async fn verify_update_and_disclosure(fixture: &Fixture) -> TestResult {
    let product = fixture.products[0];
    let updated = fixture
        .success(
            &fixture.full,
            "patchWatchlistProduct",
            json!({"productListingId":product,"state":"INACTIVE_BY_USER","notifications":false}),
        )
        .await?;
    assert_eq!(
        json!({"watching":false,"notifications":false}),
        updated["userState"]["watchlist"]
    );
    let snapshot = fixture.entry_snapshot(product).await?;
    fixture
        .success(
            &fixture.full,
            "patchWatchlistProduct",
            json!({"productListingId":product,"state":"INACTIVE_BY_USER","notifications":false}),
        )
        .await?;
    assert_eq!(snapshot, fixture.entry_snapshot(product).await?);
    let updated = fixture
        .success(
            &fixture.write,
            "patchWatchlistProduct",
            json!({"productListingId":product}),
        )
        .await?;
    assert!(updated.get("userState").is_none());
    Ok(())
}

async fn verify_quota_and_removal(fixture: &Fixture) -> TestResult {
    // One active entry remains; fill the other nineteen free-tier slots via the real use case.
    for product in &fixture.products[3..22] {
        fixture
            .success(
                &fixture.full,
                "addWatchlistProduct",
                json!({"productListingId":product}),
            )
            .await?;
    }
    fixture
        .error(
            "addWatchlistProduct",
            json!({"productListingId":fixture.products[22]}),
            "WATCHLIST_QUOTA_EXCEEDED",
        )
        .await?;
    fixture
        .error(
            "patchWatchlistProduct",
            json!({"productListingId":fixture.products[0],"state":"ACTIVE"}),
            "WATCHLIST_QUOTA_EXCEEDED",
        )
        .await?;
    let deleted = fixture
        .success(
            &fixture.full,
            "deleteWatchlistProduct",
            json!({"productListingId":fixture.products[1]}),
        )
        .await?;
    assert_eq!(json!({"result":null}), deleted);
    fixture
        .error(
            "deleteWatchlistProduct",
            json!({"productListingId":fixture.products[1]}),
            "NOT_FOUND",
        )
        .await?;
    let activated = fixture
        .success(
            &fixture.full,
            "patchWatchlistProduct",
            json!({"productListingId":fixture.products[0],"state":"ACTIVE"}),
        )
        .await?;
    assert_eq!(
        json!({"watching":true,"notifications":false}),
        activated["userState"]["watchlist"]
    );
    Ok(())
}

async fn verify_watchlist() -> TestResult {
    let fixture = Fixture::new().await?;
    verify_add_and_read_scope(&fixture).await?;
    verify_listing_pages(&fixture).await?;
    verify_service_ownership(&fixture).await;
    verify_update_and_disclosure(&fixture).await?;
    verify_quota_and_removal(&fixture).await?;
    Ok(())
}

#[aura_integration_test(services=[BUSINESS_SCHEMA], isolated = true)]
async fn should_preserve_real_watchlist_authorization_quota_paging_and_mutation_behavior() {
    let result = verify_watchlist().await;
    assert!(result.is_ok(), "{result:?}");
}
