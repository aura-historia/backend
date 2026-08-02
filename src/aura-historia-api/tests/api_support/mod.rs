#![allow(dead_code)]

use aura_historia_api::auth::{
    ApiAuthService, AuraAccessTokenAuthenticator, AuthError, RequestMetadata, TokenAuthenticator,
    TransportPrincipal,
};
use aura_historia_api::state::{
    AppState, PartnerApplicationsState, ShopsState, UsersState, WatchlistState,
};
use aura_historia_api::{app, state};
use common::domain::Domain;
use common::postgres::SqlxUnitOfWork;
use common::product_id::ProductId;
use common::shop_id::ShopId;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation};
use shop_core::shop_type::ShopType;
use shop_partner_postgres::{
    SqlxPartnerShopApplicationReaderFactory, SqlxPartnerShopApplicationRepositoryFactory,
};
use shop_partner_service::use_cases::{
    AdminDecidePartnerShopApplicationHandler, AdminGetPartnerShopApplicationHandler,
    AdminListPartnerShopApplicationsHandler, AdminUpdatePartnerShopApplicationHandler,
    CreatePartnerShopApplicationHandler, GetPartnerShopApplicationHandler,
    ListPartnerShopApplicationsHandler, WithdrawPartnerShopApplicationHandler,
};
use shop_postgres::{SqlxPartnerShopReaderFactory, SqlxShopRepositoryFactory};
use shop_service::ports::{ShopGeocoder, ShopGeocoderError, ShopRepository, ShopRepositoryFactory};
use shop_service::use_cases::commands::create_shop::CreateShopHandler;
use shop_service::use_cases::commands::update_shop::UpdateShopHandler;
use shop_service::use_cases::queries::get_shop::GetShopHandler;
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsHandler;
use shop_service::use_cases::queries::search_shops::SearchShopsHandler;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use test_api::{get_dynamodb_client, get_postgres_client};
use url::Url;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, NewAccessToken, RawAccessToken,
    Scope,
};
use user_dynamodb::DynamoDbAccessTokenStore;
use user_service::ports::AccessTokenStore;
use user_service::use_cases::commands::change_user_role::ChangeUserRoleHandler;
use user_service::use_cases::commands::change_user_tier::ChangeUserTierHandler;
use user_service::use_cases::commands::create_access_token::CreateAccessTokenHandler;
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenHandler;
use user_service::use_cases::commands::delete_user::DeleteUserHandler;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenHandler;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileHandler;
use user_service::use_cases::queries::admin_get_user::AdminGetUserHandler;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminHandler;
use user_service::use_cases::queries::get_access_token::GetAccessTokenHandler;
use user_service::use_cases::queries::get_own_user::GetOwnUserHandler;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensHandler;
use user_service::use_cases::queries::search_users::SearchUsersHandler;
use watchlist_postgres::{SqlxWatchlistReaderFactory, SqlxWatchlistRepositoryFactory};
use watchlist_service::use_cases::{
    ListWatchlistHandler, UnwatchProductHandler, UpdateWatchlistProductHandler, WatchProductHandler,
};

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

pub fn aura_api_app() -> Pin<Box<dyn Future<Output = axum::Router> + Send>> {
    Box::pin(async { app(test_state().await) })
}

pub async fn json_response(
    response: reqwest::Response,
) -> (reqwest::StatusCode, serde_json::Value) {
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode API response: {error}"));
    (status, body)
}

pub fn assert_problem(
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

pub async fn seed_user(role: &'static str) -> UserId {
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

pub async fn seed_access_token_for(user_id: UserId, scopes: HashSet<Scope>) -> RawAccessToken {
    let client = get_dynamodb_client().await;
    let store = DynamoDbAccessTokenStore::new(client, "table_1");
    let raw = RawAccessToken::new();
    let token = AccessToken::create(NewAccessToken {
        id: AccessTokenId::new(),
        hashed_token: raw.clone().into(),
        user_id,
        name: AccessTokenName::from("api acceptance"),
        scopes,
        origin: AccessTokenOrigin::User,
        expires: None,
    });
    if let Err(error) = store.insert(token).await {
        panic!("failed to seed access token: {error:?}");
    }
    raw
}

pub async fn seed_shop() -> Shop {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let repositories = SqlxShopRepositoryFactory::new();
    let id = ShopId::new();
    let shop = Shop::create(NewShop {
        id,
        name: common::shop_name::ShopName::from(format!("API Acceptance Shop {id}").as_str()),
        shop_type: ShopType::CommercialDealer,
        domains: HashSet::from([domain(format!("api-acceptance-{id}.example").as_str())]),
        shopify: None,
        woocommerce: None,
        presentation: ShopPresentation {
            url: Some(url("https://api-acceptance.example/")),
            image: None,
        },
        address: None,
        contact: ShopContact::default(),
        partner_status: ShopPartnerStatus::Partnered,
        affiliate_configuration: None,
    });

    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin shop seed transaction: {error}"));
    if let Err(error) = repositories.in_transaction(&mut tx).insert(&shop).await {
        panic!("failed to insert shop: {error:?}");
    }
    if let Err(error) = tx.commit().await {
        panic!("failed to commit shop seed transaction: {error}");
    }
    shop
}

pub async fn seed_product() -> ProductId {
    let shop = seed_shop().await;
    let product_id = ProductId::new();
    let event_id = uuid::Uuid::new_v4();
    let pool = get_postgres_client().await;
    let mut tx = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin product seed transaction: {error}"));
    if let Err(error) = sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await
    {
        panic!("failed to defer product seed constraints: {error}");
    }
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO products (
            product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id,
            state, lifecycle, url
        ) VALUES ($1, $2, $3, $4, $4, $5, 'AVAILABLE', 'PUBLISHED', $6)
        "#,
    )
    .bind(uuid::Uuid::from(product_id))
    .bind(format!("acceptance-product-{product_id}"))
    .bind(event_id)
    .bind(uuid::Uuid::from(shop.id()))
    .bind(format!("shops-product-{product_id}"))
    .bind("https://api-acceptance.example/product")
    .execute(&mut *tx)
    .await
    {
        panic!("failed to seed product: {error}");
    }
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time)
        VALUES ($1, $2, 'CREATED', 'DOMAIN', '{}', now())
        "#,
    )
    .bind(event_id)
    .bind(uuid::Uuid::from(product_id))
    .execute(&mut *tx)
    .await
    {
        panic!("failed to seed product event: {error}");
    }
    if let Err(error) = tx.commit().await {
        panic!("failed to commit product seed transaction: {error}");
    }
    product_id
}

fn domain(value: &str) -> Domain {
    match Domain::try_from(value) {
        Ok(domain) => domain,
        Err(error) => panic!("invalid test domain: {error}"),
    }
}

fn url(value: &str) -> Url {
    match Url::parse(value) {
        Ok(url) => url,
        Err(error) => panic!("invalid test URL: {error}"),
    }
}

async fn test_state() -> AppState {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let client = get_dynamodb_client().await;
    let access_token_store = DynamoDbAccessTokenStore::new(client, "table_1");
    let access_token_use_case =
        user_service::use_cases::AuthenticateAccessTokenHandler::new(access_token_store.clone());
    let authenticator = Arc::new(ApiAuthService::new(
        RejectJwtAuthenticator,
        AuraAccessTokenAuthenticator::new(access_token_use_case),
    ));

    let shops_state = ShopsState::new(
        Arc::new(GetShopHandler::new(
            unit_of_work.clone(),
            shop_postgres::SqlxShopDetailsReaderFactory::new(),
        )),
        Arc::new(SearchShopsHandler::new(
            unit_of_work.clone(),
            shop_postgres::SqlxShopSearchReaderFactory::new(),
        )),
        Arc::new(CreateShopHandler::new(
            unit_of_work.clone(),
            SqlxShopRepositoryFactory::new(),
            RejectGeocoder,
            CheckUserAdminHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAdminReaderFactory::new(),
            ),
        )),
        Arc::new(UpdateShopHandler::new(
            unit_of_work.clone(),
            SqlxShopRepositoryFactory::new(),
            RejectGeocoder,
            CheckUserAdminHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAdminReaderFactory::new(),
            ),
            SqlxPartnerShopReaderFactory::new(),
        )),
        Arc::new(ListUserPartnerShopsHandler::new(
            unit_of_work.clone(),
            SqlxPartnerShopReaderFactory::new(),
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let users_state = UsersState::new(
        Arc::new(GetOwnUserHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAccountReaderFactory::new(),
        )),
        Arc::new(AdminGetUserHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAccountReaderFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(SearchUsersHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserSearchReaderFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(UpdateUserProfileHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(ChangeUserRoleHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(ChangeUserTierHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(DeleteUserHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(CreateAccessTokenHandler::new(access_token_store.clone())),
        Arc::new(ListAccessTokensHandler::new(access_token_store.clone())),
        Arc::new(GetAccessTokenHandler::new(access_token_store.clone())),
        Arc::new(UpdateAccessTokenHandler::new(access_token_store.clone())),
        Arc::new(DeleteAccessTokenHandler::new(access_token_store)),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let watchlist_state = WatchlistState::new(
        Arc::new(ListWatchlistHandler::new(
            unit_of_work.clone(),
            SqlxWatchlistReaderFactory,
        )),
        Arc::new(WatchProductHandler::new(
            unit_of_work.clone(),
            SqlxWatchlistRepositoryFactory,
        )),
        Arc::new(UpdateWatchlistProductHandler::new(
            unit_of_work.clone(),
            SqlxWatchlistRepositoryFactory,
        )),
        Arc::new(UnwatchProductHandler::new(
            unit_of_work.clone(),
            SqlxWatchlistRepositoryFactory,
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let partner_state = PartnerApplicationsState::new(
        Arc::new(CreatePartnerShopApplicationHandler::new(
            unit_of_work.clone(),
            SqlxPartnerShopApplicationRepositoryFactory::new(),
            SqlxShopRepositoryFactory::new(),
            RejectGeocoder,
        )),
        Arc::new(ListPartnerShopApplicationsHandler::new(
            unit_of_work.clone(),
            SqlxPartnerShopApplicationReaderFactory::new(),
        )),
        Arc::new(GetPartnerShopApplicationHandler::new(
            unit_of_work.clone(),
            SqlxPartnerShopApplicationRepositoryFactory::new(),
        )),
        Arc::new(WithdrawPartnerShopApplicationHandler::new(
            unit_of_work.clone(),
            SqlxPartnerShopApplicationRepositoryFactory::new(),
        )),
        Arc::new(AdminListPartnerShopApplicationsHandler::new(
            unit_of_work.clone(),
            SqlxPartnerShopApplicationReaderFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(AdminGetPartnerShopApplicationHandler::new(
            unit_of_work.clone(),
            SqlxPartnerShopApplicationRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(AdminUpdatePartnerShopApplicationHandler::new(
            unit_of_work.clone(),
            SqlxPartnerShopApplicationRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(AdminDecidePartnerShopApplicationHandler::new(
            unit_of_work,
            SqlxPartnerShopApplicationRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    state::AppState::new(shops_state, users_state, watchlist_state, partner_state)
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
