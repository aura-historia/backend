use application::transaction::{Transaction, UnitOfWork};
use aura_historia_api::auth::{
    ApiAuthService, AuraAccessTokenAuthenticator, AuthError, RequestMetadata, TokenAuthenticator,
    TransportPrincipal,
};
use aura_historia_api::state::ShopsState;
use aura_historia_api::{app, state::AppState};
use common::user_id::UserId;
use geo::{Geocoder, GeocodingError};
use platform_postgres::SqlxUnitOfWork;
use shop_core::domain::Domain;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{
    NewShop, Shop, ShopContact, ShopPresentation, ShopifyIntegration, WoocommerceIntegration,
};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_type::ShopType;
use shop_core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop_postgres::SqlxShopRepositoryFactory;
use shop_service::ports::{ShopRepository, ShopRepositoryFactory};
use shop_service::use_cases::commands::create_shop::CreateShopHandler;
use shop_service::use_cases::commands::update_shop::UpdateShopHandler;
use shop_service::use_cases::queries::get_shop::GetShopHandler;
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsHandler;
use shop_service::use_cases::queries::search_shops::SearchShopsHandler;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use test_api::{
    AuraHistoriaApi, DynamoDB, IntegrationTestService, Postgres, aura_integration_test,
    get_dynamodb_client, get_postgres_client,
};
use url::Url;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, NewAccessToken, RawAccessToken,
    Scope,
};
use user_dynamodb::DynamoDbAccessTokenStore;
use user_service::ports::AccessTokenStore;
use user_service::use_cases::AuthenticateAccessTokenHandler;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminHandler;

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
const DYNAMODB: DynamoDB = DynamoDB();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(aura_api_app);

#[derive(Clone, Copy)]
struct RejectGeocoder;

#[async_trait::async_trait]
impl Geocoder for RejectGeocoder {
    async fn geocode(
        &self,
        _address: &shop_core::address::StructuredAddress,
    ) -> Result<shop_core::address::GeoAddress, GeocodingError> {
        Err(GeocodingError::temporarily_unavailable(
            std::io::Error::other("geocoding unavailable"),
        ))
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_get_shop_by_id_with_aura_access_token() {
    let shop = seed_shop().await;
    let token = seed_access_token().await;

    let response = match reqwest::Client::new()
        .get(format!(
            "{}/api/v1/shops/{}",
            AURA_API.base_url(),
            shop.id()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => panic!("failed to call shop API: {error}"),
    };
    let status = response.status();
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .cloned();
    let body = match response.json::<serde_json::Value>().await {
        Ok(body) => body,
        Err(error) => panic!("failed to decode shop API response: {error}"),
    };

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(
        Some(reqwest::header::HeaderValue::from_static("no-store")),
        cache_control
    );
    assert_eq!(serde_json::json!(shop.id().to_string()), body["shopId"]);
    assert_eq!(
        serde_json::json!("api-integration-shop"),
        body["shopSlugId"]
    );
    assert_eq!(serde_json::json!("EUR"), body["woocommerceCurrency"]);
    assert_eq!(serde_json::json!("de"), body["woocommerceLanguage"]);
    assert!(body.get("createdBy").is_none());
    assert!(body.get("updatedBy").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_get_shop_by_slug() {
    let shop = seed_shop().await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/by-slug/shops/api-integration-shop",
            AURA_API.base_url()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode shop API response: {error}"));

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(shop.id().to_string()), body["shopId"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_search_shops() {
    let shop = seed_shop().await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/shops?shopNameQuery=Integration&sort=name&order=asc",
            AURA_API.base_url()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode shop API response: {error}"));

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(
        serde_json::json!(shop.id().to_string()),
        body["items"][0]["shopId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_create_shop_with_admin_access_token() {
    let user_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        user_id,
        HashSet::from([Scope::ShopsRead, Scope::ShopsWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/shops", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "name": "Created API Shop",
            "shopType": "COMMERCIAL_DEALER",
            "domains": ["created-api-shop.example"],
            "url": "https://created-api-shop.example/"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode shop API response: {error}"));

    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(serde_json::json!("created-api-shop"), body["shopSlugId"]);
    assert!(body.get("createdBy").is_none());
    assert!(body.get("updatedBy").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_update_partner_shop_with_aura_access_token() {
    let shop = seed_shop().await;
    let user_id = seed_user("USER").await;
    seed_partner_shop(user_id, shop.id()).await;
    let token = seed_access_token_for(
        user_id,
        HashSet::from([Scope::ShopsWrite, Scope::PartnerShopsRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/shops/{}",
            AURA_API.base_url(),
            shop.id()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "url": "https://updated-api-shop.example/"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode shop API response: {error}"));

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(
        serde_json::json!("https://updated-api-shop.example/"),
        body["url"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_get_partner_shops_with_aura_access_token() {
    let shop = seed_shop().await;
    let user_id = seed_user("USER").await;
    seed_partner_shop(user_id, shop.id()).await;
    let token = seed_access_token_for(user_id, HashSet::from([Scope::PartnerShopsRead])).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/partner-shops", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode shop API response: {error}"));

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(shop.id().to_string()), body[0]["shopId"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_invalid_and_missing_shop_reads() {
    let client = reqwest::Client::new();

    let bad_uuid = client
        .get(format!("{}/api/v1/shops/not-a-uuid", AURA_API.base_url()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body) = json_response(bad_uuid).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );

    let missing = client
        .get(format!(
            "{}/api/v1/shops/{}",
            AURA_API.base_url(),
            ShopId::new()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body) = json_response(missing).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "SHOP_NOT_FOUND",
    );

    let invalid_token = client
        .get(format!(
            "{}/api/v1/shops/{}",
            AURA_API.base_url(),
            ShopId::new()
        ))
        .bearer_auth("invalid-token")
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body) = json_response(invalid_token).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_validate_shop_search_query() {
    let client = reqwest::Client::new();

    for (query, expected) in [
        ("searchAfter=bad", "INVALID_UUID"),
        ("sort=bad&order=asc", "BAD_SORT_VALUE"),
        ("sort=name&order=sideways", "BAD_ORDER_VALUE"),
    ] {
        let response = client
            .get(format!("{}/api/v1/shops?{query}", AURA_API.base_url()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
        let (status, body) = json_response(response).await;
        assert_problem(status, &body, reqwest::StatusCode::BAD_REQUEST, expected);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_create_shop_when_auth_body_or_policy_invalid() {
    let client = reqwest::Client::new();
    let body = create_shop_body("Rejected API Shop", "rejected-api-shop.example");

    let missing_token = client
        .post(format!("{}/api/v1/shops", AURA_API.base_url()))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body_json) = json_response(missing_token).await;
    assert_problem(
        status,
        &body_json,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let user_id = seed_user("USER").await;
    let user_token = seed_access_token_for(user_id, HashSet::from([Scope::ShopsWrite])).await;
    let forbidden = client
        .post(format!("{}/api/v1/shops", AURA_API.base_url()))
        .bearer_auth(String::from(user_token))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body_json) = json_response(forbidden).await;
    assert_problem(
        status,
        &body_json,
        reqwest::StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );

    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(admin_id, HashSet::from([Scope::ShopsWrite])).await;
    let empty_body = client
        .post(format!("{}/api/v1/shops", AURA_API.base_url()))
        .bearer_auth(String::from(admin_token))
        .body("")
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body_json) = json_response(empty_body).await;
    assert_problem(
        status,
        &body_json,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_duplicate_shop_create() {
    let client = reqwest::Client::new();
    let user_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(user_id, HashSet::from([Scope::ShopsWrite])).await;
    let name = format!("Duplicate API Shop {}", UserId::new());
    let domain = format!("{}.example", UserId::new());
    let body = create_shop_body(&name, &domain);

    let created = client
        .post(format!("{}/api/v1/shops", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let duplicate = client
        .post(format!("{}/api/v1/shops", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body_json) = json_response(duplicate).await;
    assert_problem(
        status,
        &body_json,
        reqwest::StatusCode::CONFLICT,
        "SHOP_EXISTS_ALREADY",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_invalid_update_shop_requests() {
    let client = reqwest::Client::new();
    let shop = seed_shop().await;
    let body = serde_json::json!({"url": "https://forbidden-update.example/"});

    let missing_token = client
        .patch(format!(
            "{}/api/v1/shops/{}",
            AURA_API.base_url(),
            shop.id()
        ))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body_json) = json_response(missing_token).await;
    assert_problem(
        status,
        &body_json,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let bad_uuid = client
        .patch(format!("{}/api/v1/shops/bad", AURA_API.base_url()))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body_json) = json_response(bad_uuid).await;
    assert_problem(
        status,
        &body_json,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );

    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        HashSet::from([Scope::ShopsWrite, Scope::PartnerShopsRead]),
    )
    .await;
    let forbidden = client
        .patch(format!(
            "{}/api/v1/shops/{}",
            AURA_API.base_url(),
            shop.id()
        ))
        .bearer_auth(String::from(token))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body_json) = json_response(forbidden).await;
    assert_problem(
        status,
        &body_json,
        reqwest::StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );

    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(admin_id, HashSet::from([Scope::ShopsWrite])).await;
    let missing_shop = client
        .patch(format!(
            "{}/api/v1/shops/{}",
            AURA_API.base_url(),
            ShopId::new()
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body_json) = json_response(missing_shop).await;
    assert_problem(
        status,
        &body_json,
        reqwest::StatusCode::NOT_FOUND,
        "SHOP_NOT_FOUND",
    );

    let empty_body = client
        .patch(format!(
            "{}/api/v1/shops/{}",
            AURA_API.base_url(),
            shop.id()
        ))
        .bearer_auth(String::from(admin_token))
        .body("")
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body_json) = json_response(empty_body).await;
    assert_problem(
        status,
        &body_json,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_require_auth_and_return_empty_partner_shops() {
    let client = reqwest::Client::new();

    let missing_token = client
        .get(format!("{}/api/v1/me/partner-shops", AURA_API.base_url()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body_json) = json_response(missing_token).await;
    assert_problem(
        status,
        &body_json,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, HashSet::from([Scope::PartnerShopsRead])).await;
    let empty = client
        .get(format!("{}/api/v1/me/partner-shops", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call shop API: {error}"));
    let (status, body) = json_response(empty).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!([]), body);
}

async fn json_response(response: reqwest::Response) -> (reqwest::StatusCode, serde_json::Value) {
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode shop API response: {error}"));
    (status, body)
}

fn assert_problem(
    status: reqwest::StatusCode,
    body: &serde_json::Value,
    expected_status: reqwest::StatusCode,
    expected_error: &str,
) {
    assert_eq!(expected_status, status);
    assert_eq!(
        serde_json::json!(u16::from(expected_status)),
        body["status"]
    );
    assert_eq!(serde_json::json!(expected_error), body["error"]);
}

fn create_shop_body(name: &str, domain: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "shopType": "COMMERCIAL_DEALER",
        "domains": [domain],
        "url": format!("https://{domain}/")
    })
}

async fn seed_shop() -> Shop {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let repositories = SqlxShopRepositoryFactory::new();
    let mut shop = Shop::create(NewShop {
        id: ShopId::new(),
        name: ShopName::from("API Integration Shop"),
        shop_type: ShopType::CommercialDealer,
        domains: HashSet::from([domain("api-integration-shop.example")]),
        shopify: Some(ShopifyIntegration {
            domain: domain("api-integration-shop.myshopify.com"),
            currency: None,
            language: None,
        }),
        woocommerce: Some(WoocommerceIntegration {
            webhook_secret: Some(WoocommerceWebhookSecret::from("secret")),
            currency: Some(money::Currency::Eur),
            language: Some(localization::Language::De),
        }),
        presentation: ShopPresentation {
            url: Some(url("https://api-integration-shop.example/")),
            image: None,
        },
        address: None,
        contact: ShopContact::default(),
        partner_status: ShopPartnerStatus::Partnered,
        affiliate_configuration: None,
    });
    let _ = shop.publish();

    let mut tx = match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin shop seed transaction: {error}"),
    };
    if let Err(error) = repositories.in_transaction(&mut tx).insert(&shop).await {
        panic!("failed to insert shop: {error:?}");
    }
    if let Err(error) = tx.commit().await {
        panic!("failed to commit shop seed transaction: {error}");
    }
    shop
}

async fn seed_access_token() -> RawAccessToken {
    seed_access_token_for(UserId::new(), HashSet::from([Scope::ShopsRead])).await
}

async fn seed_access_token_for(user_id: UserId, scopes: HashSet<Scope>) -> RawAccessToken {
    let client = get_dynamodb_client().await;
    let store = DynamoDbAccessTokenStore::new(client, "table_1");
    let raw = RawAccessToken::new();
    let token = AccessToken::create(NewAccessToken {
        id: AccessTokenId::new(),
        hashed_token: raw.clone().into(),
        user_id,
        name: AccessTokenName::from("shop api integration"),
        scopes,
        origin: AccessTokenOrigin::User,
        expires: None,
    });
    if let Err(error) = store.insert(token).await {
        panic!("failed to seed access token: {error:?}");
    }
    raw
}

async fn seed_user(role: &'static str) -> UserId {
    let user_id = UserId::new();
    let email = format!("{}@example.test", user_id);
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO users (user_id, email, tier, role)
        VALUES ($1, $2, 'FREE', $3)
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(email)
    .bind(role)
    .execute(&pool)
    .await
    {
        panic!("failed to seed user: {error}");
    }
    user_id
}

async fn seed_partner_shop(user_id: UserId, shop_id: ShopId) {
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO user_partner_shops (user_id, shop_id)
        VALUES ($1, $2)
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::from(shop_id))
    .execute(&pool)
    .await
    {
        panic!("failed to seed partner shop: {error}");
    }
}

fn domain(value: &str) -> Domain {
    match Domain::try_from(value) {
        Ok(domain) => domain,
        Err(error) => panic!("invalid test domain: {error}"),
    }
}

fn aura_api_app() -> Pin<Box<dyn Future<Output = axum::Router> + Send>> {
    Box::pin(async { app(test_state().await) })
}

async fn test_state() -> AppState {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let get_shop = GetShopHandler::new(
        unit_of_work.clone(),
        shop_postgres::SqlxShopDetailsReaderFactory::new(),
    );
    let search_shops = SearchShopsHandler::new(
        unit_of_work.clone(),
        shop_postgres::SqlxShopSearchReaderFactory::new(),
    );
    let check_user_admin = CheckUserAdminHandler::new(
        unit_of_work.clone(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
    );
    let create_shop = CreateShopHandler::new(
        unit_of_work.clone(),
        SqlxShopRepositoryFactory::new(),
        RejectGeocoder,
        check_user_admin,
    );
    let update_shop = UpdateShopHandler::new(
        unit_of_work.clone(),
        SqlxShopRepositoryFactory::new(),
        RejectGeocoder,
        CheckUserAdminHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        ),
        shop_postgres::SqlxPartnerShopReaderFactory::new(),
    );
    let list_user_partner_shops = ListUserPartnerShopsHandler::new(
        unit_of_work,
        shop_postgres::SqlxPartnerShopReaderFactory::new(),
    );
    let client = get_dynamodb_client().await;
    let store = DynamoDbAccessTokenStore::new(client, "table_1");
    let access_token = AuthenticateAccessTokenHandler::new(store);
    let authenticator = std::sync::Arc::new(ApiAuthService::new(
        RejectJwtAuthenticator,
        AuraAccessTokenAuthenticator::new(access_token),
    ));
    AppState::with_shops_only(ShopsState::new(
        std::sync::Arc::new(get_shop),
        std::sync::Arc::new(search_shops),
        std::sync::Arc::new(create_shop),
        std::sync::Arc::new(update_shop),
        std::sync::Arc::new(list_user_partner_shops),
        authenticator,
    ))
}

struct RejectJwtAuthenticator;

#[async_trait::async_trait]
impl TokenAuthenticator for RejectJwtAuthenticator {
    async fn authenticate(
        &self,
        _bearer_token: &str,
        _metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        Err(AuthError::InvalidCredentials)
    }
}

fn url(value: &str) -> Url {
    match Url::parse(value) {
        Ok(url) => url,
        Err(error) => panic!("invalid test URL: {error}"),
    }
}
