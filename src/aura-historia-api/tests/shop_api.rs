use aura_historia_api::auth::{
    ApiAuthService, AuraAccessTokenAuthenticator, AuthError, RequestMetadata, TokenAuthenticator,
    TransportPrincipal,
};
use aura_historia_api::shops::ShopsState;
use aura_historia_api::{AppState, app};
use common::domain::Domain;
use common::postgres::SqlxUnitOfWork;
use common::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, shop_name::ShopName, user_id::UserId};
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation, WoocommerceIntegration};
use shop_core::shop_type::ShopType;
use shop_core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop_postgres::SqlxShopRepositoryFactory;
use shop_service::ports::{ShopRepository, ShopRepositoryFactory};
use shop_service::use_cases::queries::get_shop::GetShopHandler;
use std::collections::HashSet;
use test_api::{
    DynamoDB, IntegrationTestService, Postgres, aura_integration_test, get_dynamodb_client,
    get_postgres_client,
};
use time::OffsetDateTime;
use url::Url;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, RawAccessToken, Scope,
};
use user_dynamodb::DynamoDbAccessTokenStore;
use user_service::ports::AccessTokenStore;
use user_service::use_cases::AuthenticateAccessTokenHandler;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const DYNAMODB: DynamoDB = DynamoDB();

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB])]
async fn should_get_shop_by_id_with_aura_access_token() {
    let shop = seed_shop().await;
    let token = seed_access_token().await;
    let state = test_state().await;
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("failed to bind listener: {error}"),
    };
    let addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(error) => panic!("failed to get listener address: {error}"),
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(aura_historia_api::serve(listener, app(state), async move {
        let _ = shutdown_rx.await;
    }));

    let response = match reqwest::Client::new()
        .get(format!("http://{addr}/api/v1/shops/{}", shop.id()))
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
    let _send_result = shutdown_tx.send(());
    match server.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("server failed: {error}"),
        Err(error) => panic!("server task failed: {error}"),
    }

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

async fn seed_shop() -> Shop {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let repositories = SqlxShopRepositoryFactory::new();
    let shop = Shop::create(NewShop {
        id: ShopId::new(),
        name: ShopName::from("API Integration Shop"),
        shop_type: ShopType::CommercialDealer,
        domains: HashSet::from([domain("api-integration-shop.example")]),
        shopify: None,
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
    let client = get_dynamodb_client().await;
    let store = DynamoDbAccessTokenStore::new(client, "table_1");
    let raw = RawAccessToken::new();
    let now = OffsetDateTime::now_utc();
    let token = AccessToken {
        id: AccessTokenId::new(),
        hashed_token: raw.clone().into(),
        user_id: UserId::new(),
        name: AccessTokenName::from("shop api integration"),
        scopes: HashSet::from([Scope::ShopsRead]),
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

fn domain(value: &str) -> Domain {
    match Domain::try_from(value) {
        Ok(domain) => domain,
        Err(error) => panic!("invalid test domain: {error}"),
    }
}

async fn test_state() -> AppState {
    let pool = get_postgres_client().await;
    let get_shop = GetShopHandler::new(
        SqlxUnitOfWork::new(pool),
        shop_postgres::SqlxShopDetailsReaderFactory::new(),
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
