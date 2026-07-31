use aura_historia_api::auth::{
    ApiAuthService, AuraAccessTokenAuthenticator, AuthError, RequestMetadata, TokenAuthenticator,
    TransportPrincipal,
};
use aura_historia_api::shop_write_policy::ShopWritePolicyAdapter;
use aura_historia_api::state::ShopsState;
use aura_historia_api::{app, state::AppState};
use common::domain::Domain;
use common::postgres::SqlxUnitOfWork;
use common::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, shop_name::ShopName, user_id::UserId};
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{
    NewShop, Shop, ShopContact, ShopPresentation, ShopifyIntegration, WoocommerceIntegration,
};
use shop_core::shop_type::ShopType;
use shop_core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop_postgres::SqlxShopRepositoryFactory;
use shop_service::ports::{ShopGeocoder, ShopGeocoderError};
use shop_service::ports::{ShopRepository, ShopRepositoryFactory};
use shop_service::use_cases::commands::create_shop::CreateShopHandler;
use shop_service::use_cases::commands::update_shop::UpdateShopHandler;
use shop_service::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopHandler;
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
use time::OffsetDateTime;
use url::Url;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, RawAccessToken, Scope,
};
use user_dynamodb::DynamoDbAccessTokenStore;
use user_service::ports::AccessTokenStore;
use user_service::use_cases::AuthenticateAccessTokenHandler;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminHandler;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const DYNAMODB: DynamoDB = DynamoDB();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(aura_api_app);

#[derive(Clone, Copy)]
struct RejectGeocoder;

#[async_trait::async_trait]
impl ShopGeocoder for RejectGeocoder {
    async fn geocode(
        &self,
        _address: &shop_core::address::StructuredAddress,
    ) -> Result<shop_core::address::GeoAddress, ShopGeocoderError> {
        Err(ShopGeocoderError::TemporarilyUnavailable)
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

async fn seed_shop() -> Shop {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let repositories = SqlxShopRepositoryFactory::new();
    let shop = Shop::create(NewShop {
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
            currency: Some(common::currency::domain::Currency::Eur),
            language: Some(common::language::domain::Language::De),
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
    let now = OffsetDateTime::now_utc();
    let token = AccessToken {
        id: AccessTokenId::new(),
        hashed_token: raw.clone().into(),
        user_id,
        name: AccessTokenName::from("shop api integration"),
        scopes,
        origin: AccessTokenOrigin::User,
        expires: None,
        created: now,
        updated: now,
    };
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
    let check_user_partner_shop = CheckUserPartnerShopHandler::new(
        unit_of_work.clone(),
        shop_postgres::SqlxPartnerShopReaderFactory::new(),
    );
    let write_policy = ShopWritePolicyAdapter::new(check_user_admin, check_user_partner_shop);
    let create_shop = CreateShopHandler::new(
        unit_of_work.clone(),
        SqlxShopRepositoryFactory::new(),
        shop_postgres::SqlxShopDetailsReaderFactory::new(),
        RejectGeocoder,
        write_policy.clone(),
    );
    let update_shop = UpdateShopHandler::new(
        unit_of_work.clone(),
        SqlxShopRepositoryFactory::new(),
        shop_postgres::SqlxShopDetailsReaderFactory::new(),
        RejectGeocoder,
        write_policy,
    );
    let list_user_partner_shops = ListUserPartnerShopsHandler::new(
        unit_of_work,
        shop_postgres::SqlxPartnerShopReaderFactory::new(),
    );
    let client = get_dynamodb_client().await;
    let store = DynamoDbAccessTokenStore::new(client, "table_1");
    let access_token = AuthenticateAccessTokenHandler::new(store);
    let authenticator = ApiAuthService::new(
        RejectJwtAuthenticator,
        AuraAccessTokenAuthenticator::new(access_token),
    );
    AppState::new(ShopsState::new(
        std::sync::Arc::new(get_shop),
        std::sync::Arc::new(search_shops),
        std::sync::Arc::new(create_shop),
        std::sync::Arc::new(update_shop),
        std::sync::Arc::new(list_user_partner_shops),
        std::sync::Arc::new(authenticator),
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
