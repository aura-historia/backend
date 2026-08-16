#![allow(dead_code)]

use aura_historia_api::auth::{
    ApiAuthService, AuraAccessTokenAuthenticator, AuthError, RequestMetadata, TokenAuthenticator,
    TransportPrincipal,
};
use aura_historia_api::state::{
    AppState, BillingState, NewsletterState, OAuthState, PartnerApplicationsState,
    PartnerProductsState, ProductsState, SearchFiltersState, ShopsState, UsersState,
    WatchlistState, WebhooksState,
};
use aura_historia_api::{app, state};
use billing_service::ports::{
    CreateStripeCheckoutSessionRequest, CreateStripeCustomerRequest,
    CreateStripePortalSessionRequest, StripeBillingError, StripeCheckoutSessionCreator,
    StripeCustomerCreator, StripePortalSessionCreator,
};
use billing_service::use_cases::{
    BillingPriceIds, CreateBillingCheckoutSessionHandler, CreateBillingManagementSessionHandler,
    CreateBillingPortalSessionHandler,
};
use common::domain::Domain;
use common::fx_rate_id::FxRateId;
use common::postgres::SqlxUnitOfWork;
use common::product_id::ProductId;
use common::shop_id::ShopId;
use common::stripe_customer_id::StripeCustomerId;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use embedding::{
    EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
};
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use geo::{Geocoder, GeocodingError};
use notification_dynamodb::all_notifications_reader::DynamoDbAllNotificationsReader;
use notification_dynamodb::conditional_writer::ConditionalDynamoDbNotificationWriter;
use notification_dynamodb::product_notifications_reader::DynamoDbProductNotificationsReader;
use notification_service::use_cases::commands::create_notification::CreateNotificationHandler;
use oauth_dynamodb::repository::OAuthDynamoDbStore;
use oauth_service::access_token_gateway::StoreOAuthAccessTokenGateway;
use oauth_service::use_cases::{
    AuthorizeHandler, CreateOAuthClientHandler, DeleteOAuthClientHandler, GetOAuthClientHandler,
    IntrospectTokenHandler, ListOAuthClientsHandler, RevokeTokenHandler,
    TokenByAuthorizationCodeHandler, TokenByThirdPartyCodeHandler, UpdateOAuthClientHandler,
};
use product_opensearch::{OpenSearchProductSearchReader, OpenSearchProductSimilarProductsReader};
use product_postgres::{
    SqlxPartnerProductAuthorizerFactory, SqlxProductDetailsBatchReader,
    SqlxProductDetailsReaderFactory, SqlxProductEmbeddingReaderFactory,
    SqlxProductEventReaderFactory, SqlxProductEventStoreFactory, SqlxProductRepositoryFactory,
    SqlxProductUserStateReader, SqlxProductWatchlistDetailsReaderFactory,
};

use product_service::use_cases::{
    CreateProductHandler, DeleteProductHandler, GetProductEventsHandler, GetProductHandler,
    GetSimilarProductsHandler, IngestWoocommerceProductHandler, SearchProductsHandler,
    UpdateProductHandler, UpsertProductHandler,
};
use search_filter_postgres::{
    SqlxSearchFilterMatchRepositoryFactory, SqlxSearchFilterQuotaReaderFactory,
    SqlxSearchFilterReader, SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::use_cases::{
    CreateSearchFilterHandler, DeleteOwnedSearchFilterHandler, GetOwnedSearchFilterHandler,
    ListOwnedSearchFiltersHandler, ListSearchFilterMatchesHandler, UpdateOwnedSearchFilterHandler,
    UpdateSearchFilterMatchFeedbackHandler,
};
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation};
use shop_core::shop_type::ShopType;
use shop_partner_postgres::{
    SqlxPartnerShopApplicationReaderFactory, SqlxPartnerShopApplicationRepositoryFactory,
    SqlxUserPartnerShopMembershipRepositoryFactory,
};
use shop_partner_service::use_cases::{
    AdminDecidePartnerShopApplicationHandler, AdminGetPartnerShopApplicationHandler,
    AdminListPartnerShopApplicationsHandler, AdminUpdatePartnerShopApplicationHandler,
    CreatePartnerShopApplicationHandler, GetPartnerShopApplicationHandler,
    ListPartnerShopApplicationsHandler, WithdrawPartnerShopApplicationHandler,
};
use shop_postgres::{
    SqlxPartnerShopReaderFactory, SqlxShopRepositoryFactory,
    SqlxWoocommerceWebhookShopReaderFactory, SqlxWoocommerceWebhookSignatureVerifierFactory,
};
use shop_service::ports::{ShopRepository, ShopRepositoryFactory};
use shop_service::use_cases::commands::create_shop::CreateShopHandler;
use shop_service::use_cases::commands::update_shop::UpdateShopHandler;
use shop_service::use_cases::queries::get_shop::GetShopHandler;
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsHandler;
use shop_service::use_cases::queries::search_shops::SearchShopsHandler;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use test_api::{get_dynamodb_client, get_opensearch_client, get_postgres_client};
use url::Url;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, NewAccessToken, RawAccessToken,
    Scope,
};
use user_core::tier::UserTier;
use user_dynamodb::DynamoDbAccessTokenStore;
use user_service::ports::{
    AccessTokenStore, NewsletterSubscriptionWriteError, NewsletterSubscriptionWriter,
};
use user_service::use_cases::commands::associate_user_stripe_customer_id::AssociateUserStripeCustomerIdHandler;
use user_service::use_cases::commands::change_user_role::ChangeUserRoleHandler;
use user_service::use_cases::commands::change_user_tier::ChangeUserTierHandler;
use user_service::use_cases::commands::create_access_token::CreateAccessTokenHandler;
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenHandler;
use user_service::use_cases::commands::delete_user::DeleteUserHandler;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenHandler;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileHandler;
use user_service::use_cases::commands::upsert_newsletter_subscription::UpsertNewsletterSubscriptionHandler;
use user_service::use_cases::queries::admin_get_user::AdminGetUserHandler;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminHandler;
use user_service::use_cases::queries::get_access_token::GetAccessTokenHandler;
use user_service::use_cases::queries::get_own_user::GetOwnUserHandler;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensHandler;
use user_service::use_cases::queries::search_users::SearchUsersHandler;
use watchlist_postgres::{SqlxWatchlistQuotaReaderFactory, SqlxWatchlistRepositoryFactory};
use watchlist_service::use_cases::{
    ListWatchlistHandler, UnwatchProductHandler, UpdateWatchlistProductHandler, WatchProductHandler,
};

#[derive(Clone, Copy)]
enum TestEmbeddingGenerator {
    Success,
    Failure,
}

impl TestEmbeddingGenerator {
    fn embedding(&self) -> Result<EmbeddingVector, EmbeddingError> {
        match self {
            Self::Success => EmbeddingVector::try_new(vec![1.0; embedding::EMBEDDING_DIMENSIONS]),
            Self::Failure => Err(EmbeddingError::InvalidInput {
                reason: "test embedding failure",
            }),
        }
    }
}

#[async_trait::async_trait]
impl EmbeddingGenerator for TestEmbeddingGenerator {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.embedding()
    }

    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.embedding()
    }
}

#[derive(Clone, Copy)]
struct TestStripeBilling;

#[async_trait::async_trait]
impl StripeCustomerCreator for TestStripeBilling {
    async fn create_customer(
        &self,
        request: CreateStripeCustomerRequest,
    ) -> Result<StripeCustomerId, StripeBillingError> {
        Ok(StripeCustomerId::from(format!("cus_{}", request.user_id)))
    }
}

#[async_trait::async_trait]
impl StripeCheckoutSessionCreator for TestStripeBilling {
    async fn create_checkout_session(
        &self,
        request: CreateStripeCheckoutSessionRequest,
    ) -> Result<Url, StripeBillingError> {
        Ok(url(&format!(
            "https://checkout.stripe.test/{}/{}",
            request.stripe_customer_id, request.price_id
        )))
    }
}

#[async_trait::async_trait]
impl StripePortalSessionCreator for TestStripeBilling {
    async fn create_portal_session(
        &self,
        request: CreateStripePortalSessionRequest,
    ) -> Result<Url, StripeBillingError> {
        Ok(url(&format!(
            "https://billing.stripe.test/{}",
            request.stripe_customer_id
        )))
    }
}

#[derive(Clone, Copy)]
struct SuccessfulNewsletterWriter;

#[async_trait::async_trait]
impl NewsletterSubscriptionWriter for SuccessfulNewsletterWriter {
    async fn upsert(
        &self,
        _subscription: &user_core::newsletter_subscription::NewsletterSubscription,
    ) -> Result<(), NewsletterSubscriptionWriteError> {
        Ok(())
    }
}

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

pub fn aura_api_app() -> Pin<Box<dyn Future<Output = axum::Router> + Send>> {
    Box::pin(async { app(test_state(TestEmbeddingGenerator::Success).await) })
}

pub fn aura_api_app_with_failed_search_embedding()
-> Pin<Box<dyn Future<Output = axum::Router> + Send>> {
    Box::pin(async { app(test_state(TestEmbeddingGenerator::Failure).await) })
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
    seed_user_with_tier(role, UserTier::Free).await
}

pub async fn seed_user_with_tier(role: &'static str, tier: UserTier) -> UserId {
    let user_id = UserId::new();
    let email = format!("{}@example.test", user_id);
    let tier = match tier {
        UserTier::Free => "FREE",
        UserTier::Pro => "PRO",
        UserTier::Ultimate => "ULTIMATE",
    };
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO users (user_id, email, tier, role)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(email)
    .bind(tier)
    .bind(role)
    .execute(&pool)
    .await
    {
        panic!("failed to seed user: {error}");
    }
    user_id
}

pub async fn seed_active_watchlist_entries(user_id: UserId, count: usize) {
    for _ in 0..count {
        let product_id = seed_product().await;
        seed_watchlist_entry(user_id, product_id, "ACTIVE").await;
    }
}

pub async fn seed_inactive_watchlist_entry(user_id: UserId) -> ProductId {
    let product_id = seed_product().await;
    seed_watchlist_entry(user_id, product_id, "INACTIVE_BY_USER").await;
    product_id
}

async fn seed_watchlist_entry(user_id: UserId, product_id: ProductId, state: &'static str) {
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        "INSERT INTO product_watchlist (user_id, product_id, notifications, state) VALUES ($1, $2, true, $3)",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::from(product_id))
    .bind(state)
    .execute(&pool)
    .await
    {
        panic!("failed to seed watchlist entry: {error}");
    }
}

pub async fn seed_partner_shop(user_id: UserId, shop_id: ShopId) {
    let pool = get_postgres_client().await;
    if let Err(error) =
        sqlx::query("INSERT INTO user_partner_shops (user_id, shop_id) VALUES ($1, $2)")
            .bind(uuid::Uuid::from(user_id))
            .bind(uuid::Uuid::from(shop_id))
            .execute(&pool)
            .await
    {
        panic!("failed to seed partner-shop membership: {error}");
    }
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
    let mut shop = Shop::create(NewShop {
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
    let _ = shop.publish();

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

pub async fn product_route_slugs(product_id: ProductId) -> (String, String) {
    let pool = get_postgres_client().await;
    sqlx::query_as(
        "SELECT shops.shop_slug_id, products.product_slug_id FROM products JOIN shops ON shops.shop_id = products.shop_id WHERE products.product_id = $1",
    )
    .bind(uuid::Uuid::from(product_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to read seeded product slugs: {error}"))
}

pub async fn seed_product() -> ProductId {
    let shop = seed_shop().await;
    let product_id = ProductId::new();
    let product_slug_id = common::product_slug_id::ProductSlugId::from("acceptance-product");
    let event_id = uuid::Uuid::new_v4();
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
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
        ) VALUES ($1, $2, $3, $4, $4, $5, 'AVAILABLE', 'ACTIVE', $6)
        "#,
    )
    .bind(uuid::Uuid::from(product_id))
    .bind(product_slug_id.as_ref())
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
        VALUES (
            $1,
            $2,
            'PRODUCT_CREATED',
            'DOMAIN',
            '{"title":null,"description":null,"address":{},"pricing":{},"state":"Available","url":"https://api-acceptance.example/product","images":[],"auction":{}}',
            now()
        )
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

async fn seed_current_fx_snapshot(pool: &sqlx::PgPool) {
    let fx_rate_id = FxRateId::new();
    if let Err(error) = sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, now(), $2, $3)",
    )
    .bind(uuid::Uuid::from(fx_rate_id))
    .bind("fxratesapi")
    .bind(fx_rate_id.to_string())
    .execute(pool)
    .await
    {
        panic!("failed to seed current FX snapshot: {error}");
    }

    for currency in [
        "EUR", "GBP", "USD", "AUD", "CAD", "NZD", "CNY", "BRL", "PLN", "TRY", "JPY", "CZK", "RUB",
        "AED", "SAR", "HKD", "SGD", "CHF",
    ] {
        if let Err(error) = sqlx::query(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) VALUES ($1, $2, $3)",
        )
        .bind(uuid::Uuid::from(fx_rate_id))
        .bind(currency)
        .bind(if currency == "EUR" {
            1_000_000_i64
        } else {
            1_250_000_i64
        })
        .execute(pool)
        .await
        {
            panic!("failed to seed current FX quote: {error}");
        }
    }
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

async fn test_state(search_embeddings: TestEmbeddingGenerator) -> AppState {
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let client = get_dynamodb_client().await;
    let access_token_store = DynamoDbAccessTokenStore::new(client, "table_1");
    let access_token_use_case =
        user_service::use_cases::AuthenticateAccessTokenHandler::new(access_token_store.clone());
    let authenticator = Arc::new(ApiAuthService::new(
        RejectJwtAuthenticator,
        AuraAccessTokenAuthenticator::new(access_token_use_case),
    ));
    let oauth_store = OAuthDynamoDbStore::new(client, "table_1");
    let oauth_access_tokens = StoreOAuthAccessTokenGateway::new(access_token_store.clone());
    let opensearch_client = get_opensearch_client().await;

    let products_state = ProductsState::new(
        Arc::new(GetProductHandler::new(
            unit_of_work.clone(),
            SqlxProductDetailsReaderFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
            DynamoDbProductNotificationsReader::new(client, "table_1"),
        )),
        Arc::new(GetSimilarProductsHandler::new(
            unit_of_work.clone(),
            SqlxProductEmbeddingReaderFactory::new(),
            OpenSearchProductSimilarProductsReader::new(opensearch_client.clone()),
            SqlxProductUserStateReader::new(get_postgres_client().await),
            DynamoDbAllNotificationsReader::new(client, "table_1"),
        )),
        Arc::new(SearchProductsHandler::new(
            OpenSearchProductSearchReader::new(opensearch_client.clone()),
            search_embeddings,
            SqlxProductUserStateReader::new(get_postgres_client().await),
            DynamoDbAllNotificationsReader::new(client, "table_1"),
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    )
    .with_product_events(Arc::new(GetProductEventsHandler::new(
        unit_of_work.clone(),
        SqlxProductEventReaderFactory::new(),
    )));

    let partner_products_state = PartnerProductsState::new(
        Arc::new(CreateProductHandler::new_with_fx_rates(
            unit_of_work.clone(),
            SqlxProductRepositoryFactory::new(),
            SqlxProductEventStoreFactory::new(),
            SqlxPartnerProductAuthorizerFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
        )),
        Arc::new(UpdateProductHandler::new_with_fx_rates(
            unit_of_work.clone(),
            SqlxProductRepositoryFactory::new(),
            SqlxProductEventStoreFactory::new(),
            SqlxPartnerProductAuthorizerFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
        )),
        Arc::new(UpsertProductHandler::new_with_fx_rates(
            unit_of_work.clone(),
            SqlxProductRepositoryFactory::new(),
            SqlxProductEventStoreFactory::new(),
            SqlxPartnerProductAuthorizerFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
        )),
        Arc::new(DeleteProductHandler::new(
            unit_of_work.clone(),
            SqlxProductRepositoryFactory::new(),
            SqlxProductEventStoreFactory::new(),
            SqlxPartnerProductAuthorizerFactory::new(),
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let webhooks_state = WebhooksState::new(
        Arc::new(IngestWoocommerceProductHandler::new_with_fx_rates(
            unit_of_work.clone(),
            SqlxPartnerShopReaderFactory::new(),
            SqlxWoocommerceWebhookShopReaderFactory::new(),
            SqlxWoocommerceWebhookSignatureVerifierFactory::new(),
            SqlxProductRepositoryFactory::new(),
            SqlxProductEventStoreFactory::new(),
            SqlxPartnerProductAuthorizerFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

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
            user_postgres::SqlxUserTierEntitlementsFactory::new(),
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

    let search_filter_reader = SqlxSearchFilterReader::new(get_postgres_client().await);
    let search_filters_state = SearchFiltersState::new(
        Arc::new(ListOwnedSearchFiltersHandler::new(
            search_filter_reader.clone(),
        )),
        Arc::new(CreateSearchFilterHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
            TestEmbeddingGenerator::Success,
            SqlxSearchFilterQuotaReaderFactory,
            user_postgres::SqlxUserTierEntitlementsFactory::new(),
        )),
        Arc::new(GetOwnedSearchFilterHandler::new(
            search_filter_reader.clone(),
        )),
        Arc::new(UpdateOwnedSearchFilterHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
            TestEmbeddingGenerator::Success,
            search_filter_reader.clone(),
            SqlxSearchFilterQuotaReaderFactory,
            user_postgres::SqlxUserTierEntitlementsFactory::new(),
        )),
        Arc::new(DeleteOwnedSearchFilterHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
        )),
        Arc::new(ListSearchFilterMatchesHandler::new(
            unit_of_work.clone(),
            search_filter_reader.clone(),
            SqlxProductDetailsBatchReader::new(get_postgres_client().await),
            SqlxFxRateSnapshotRepositoryFactory,
            DynamoDbAllNotificationsReader::new(client, "table_1"),
        )),
        Arc::new(UpdateSearchFilterMatchFeedbackHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
            SqlxSearchFilterMatchRepositoryFactory,
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let watchlist_state = WatchlistState::new(
        Arc::new(ListWatchlistHandler::new(
            unit_of_work.clone(),
            SqlxProductWatchlistDetailsReaderFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
            DynamoDbAllNotificationsReader::new(client, "table_1"),
        )),
        Arc::new(WatchProductHandler::new(
            unit_of_work.clone(),
            SqlxWatchlistRepositoryFactory,
            SqlxWatchlistQuotaReaderFactory,
            user_postgres::SqlxUserTierEntitlementsFactory::new(),
        )),
        Arc::new(UpdateWatchlistProductHandler::new(
            unit_of_work.clone(),
            SqlxWatchlistRepositoryFactory,
            SqlxWatchlistQuotaReaderFactory,
            user_postgres::SqlxUserTierEntitlementsFactory::new(),
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
            SqlxShopRepositoryFactory::new(),
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
            unit_of_work.clone(),
            SqlxPartnerShopApplicationRepositoryFactory::new(),
            shop_postgres::SqlxShopRepositoryFactory::new(),
            SqlxUserPartnerShopMembershipRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
            CreateNotificationHandler::new(ConditionalDynamoDbNotificationWriter::new(
                get_dynamodb_client().await.clone(),
                "table_1",
            )),
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let billing_prices = BillingPriceIds {
        pro_monthly: "price_pro_monthly".to_owned(),
        pro_yearly: "price_pro_yearly".to_owned(),
        ultimate_monthly: "price_ultimate_monthly".to_owned(),
        ultimate_yearly: "price_ultimate_yearly".to_owned(),
    };
    let billing_state = BillingState::new(
        Arc::new(CreateBillingCheckoutSessionHandler::new(
            GetOwnUserHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAccountReaderFactory::new(),
            ),
            AssociateUserStripeCustomerIdHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserRepositoryFactory::new(),
            ),
            TestStripeBilling,
            TestStripeBilling,
            billing_prices.clone(),
        )),
        Arc::new(CreateBillingPortalSessionHandler::new(
            GetOwnUserHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAccountReaderFactory::new(),
            ),
            TestStripeBilling,
        )),
        Arc::new(CreateBillingManagementSessionHandler::new(
            GetOwnUserHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAccountReaderFactory::new(),
            ),
            AssociateUserStripeCustomerIdHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserRepositoryFactory::new(),
            ),
            TestStripeBilling,
            TestStripeBilling,
            TestStripeBilling,
            billing_prices,
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let oauth_state = OAuthState::new(
        Arc::new(CreateOAuthClientHandler::new(oauth_store.clone())),
        Arc::new(ListOAuthClientsHandler::new(oauth_store.clone())),
        Arc::new(GetOAuthClientHandler::new(oauth_store.clone())),
        Arc::new(UpdateOAuthClientHandler::new(oauth_store.clone())),
        Arc::new(DeleteOAuthClientHandler::new(oauth_store.clone())),
        Arc::new(AuthorizeHandler::new(
            oauth_store.clone(),
            oauth_store.clone(),
        )),
        Arc::new(TokenByAuthorizationCodeHandler::new(
            oauth_store.clone(),
            oauth_store.clone(),
            oauth_store.clone(),
            oauth_access_tokens.clone(),
        )),
        Arc::new(TokenByThirdPartyCodeHandler::new(oauth_store.clone())),
        Arc::new(RevokeTokenHandler::new(
            oauth_store.clone(),
            oauth_access_tokens.clone(),
        )),
        Arc::new(IntrospectTokenHandler::new(
            oauth_store,
            oauth_access_tokens,
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    state::AppState::new(shops_state, users_state, watchlist_state, partner_state)
        .with_newsletter(NewsletterState::new(
            Arc::new(UpsertNewsletterSubscriptionHandler::new(
                user_postgres::SqlxNewsletterProfileReader::new(get_postgres_client().await),
                SuccessfulNewsletterWriter,
            )),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_products(products_state)
        .with_partner_products(partner_products_state)
        .with_webhooks(webhooks_state)
        .with_oauth(oauth_state)
        .with_search_filters(search_filters_state)
        .with_billing(billing_state)
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
