pub mod auth;
pub mod billing;
pub mod error;
pub mod newsletter;
pub mod oauth;
pub mod partner_applications;
pub mod partner_products;
pub mod products;
pub mod search_filters;
pub mod shops;
pub mod state;
pub mod transport;
pub mod users;
pub mod watchlist;
pub mod webhooks;

use crate::auth::{
    ApiAuthService, AuraAccessTokenAuthenticator, AuthError, CognitoJwtAuthenticator,
    CognitoJwtConfig, JwksProvider, ReqwestJwksProvider, TokenAuthenticator,
};
use crate::state::{
    AppState, BillingState, NewsletterState, OAuthState, PartnerApplicationsState,
    PartnerProductsState, ProductsState, ReadinessCheck, SearchFiltersState, ShopsState,
    UsersState, WatchlistState, WebhooksState,
};
use crate::transport::with_transport_middleware;
use axum::Router;
use axum::routing::{delete, get, patch, post};
use billing_service::use_cases::{
    BillingPriceIds, CreateBillingCheckoutSessionHandler, CreateBillingManagementSessionHandler,
    CreateBillingPortalSessionHandler,
};
use billing_stripe::{StripeBillingClient, StripeBillingConfig};
use common::postgres::{PostgresConnectError, SqlxUnitOfWork};
use embedding::{EmbeddingGenerator, VertexAiEmbeddingConfig, VertexAiEmbeddingGenerator};
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use geo::{GoogleGeocoder, GoogleGeocoderConfig};
use google_cloud_auth::credentials::Builder as GoogleCredentialsBuilder;
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
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
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
    SqlxPartnerShopReaderFactory, SqlxShopDetailsReaderFactory, SqlxShopRepositoryFactory,
    SqlxShopSearchReaderFactory, SqlxWoocommerceWebhookShopReaderFactory,
    SqlxWoocommerceWebhookSignatureVerifierFactory,
};
use shop_service::use_cases::commands::create_shop::CreateShopHandler;
use shop_service::use_cases::commands::update_shop::UpdateShopHandler;
use shop_service::use_cases::queries::get_shop::GetShopHandler;
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsHandler;
use shop_service::use_cases::queries::search_shops::SearchShopsHandler;
use sqlx::PgPool;
use std::future::Future;
use std::net::{AddrParseError, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tracing::info;
use user_dynamodb::DynamoDbAccessTokenStore;
use user_postgres::{
    SqlxNewsletterProfileReader, SqlxUserAccountReaderFactory, SqlxUserAdminReaderFactory,
    SqlxUserRepositoryFactory, SqlxUserSearchReaderFactory, SqlxUserTierEntitlementsFactory,
};
use user_service::use_cases::AuthenticateAccessTokenHandler;
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
use user_zoho::ZohoNewsletterSubscriptionWriter;
use watchlist_postgres::{SqlxWatchlistQuotaReaderFactory, SqlxWatchlistRepositoryFactory};
use watchlist_service::use_cases::{
    ListWatchlistHandler, UnwatchProductHandler, UpdateWatchlistProductHandler, WatchProductHandler,
};

pub const API_BIND_ADDR_ENV: &str = "AURA_HISTORIA_API_BIND_ADDR";
pub const DYNAMODB_TABLE_NAME_ENV: &str = "DYNAMODB_TABLE_NAME";
pub const VERTEX_AI_PROJECT_ID_ENV: &str = "VERTEX_AI_PROJECT_ID";
pub const VERTEX_AI_LOCATION_ENV: &str = "VERTEX_AI_LOCATION";
pub const COGNITO_ISSUER_ENV: &str = "AURA_HISTORIA_COGNITO_ISSUER";
pub const COGNITO_JWKS_URL_ENV: &str = "AURA_HISTORIA_COGNITO_JWKS_URL";
pub const COGNITO_APP_CLIENT_IDS_ENV: &str = "AURA_HISTORIA_COGNITO_APP_CLIENT_IDS";
pub const GOOGLE_GEOCODING_API_KEY_ENV: &str = "GOOGLE_GEOCODING_API_KEY";
pub const STRIPE_API_KEY_ENV: &str = "STRIPE_API_KEY";
pub const STRIPE_CHECKOUT_SUCCESS_URL_ENV: &str = "STRIPE_CHECKOUT_SUCCESS_URL";
pub const STRIPE_CHECKOUT_CANCEL_URL_ENV: &str = "STRIPE_CHECKOUT_CANCEL_URL";
pub const STRIPE_PORTAL_RETURN_URL_ENV: &str = "STRIPE_PORTAL_RETURN_URL";
pub const STRIPE_PRO_MONTHLY_PRICE_ID_ENV: &str = "STRIPE_PRO_MONTHLY_PRICE_ID";
pub const STRIPE_PRO_YEARLY_PRICE_ID_ENV: &str = "STRIPE_PRO_YEARLY_PRICE_ID";
pub const STRIPE_ULTIMATE_MONTHLY_PRICE_ID_ENV: &str = "STRIPE_ULTIMATE_MONTHLY_PRICE_ID";
pub const STRIPE_ULTIMATE_YEARLY_PRICE_ID_ENV: &str = "STRIPE_ULTIMATE_YEARLY_PRICE_ID";
pub const ZOHO_LIST_KEY_ENV: &str = "ZOHO_LIST_KEY";
pub const ZOHO_CLIENT_ID_ENV: &str = "ZOHO_CLIENT_ID";
pub const ZOHO_CLIENT_SECRET_ENV: &str = "ZOHO_CLIENT_SECRET";
pub const ZOHO_REFRESH_TOKEN_ENV: &str = "ZOHO_REFRESH_TOKEN";
pub const ZOHO_ACCOUNTS_URL_ENV: &str = "ZOHO_ACCOUNTS_URL";
pub const ZOHO_CAMPAIGNS_URL_ENV: &str = "ZOHO_CAMPAIGNS_URL";
const DEFAULT_API_BIND_ADDR: &str = "0.0.0.0:8080";
const JWKS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const JWKS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_VERTEX_AI_PROJECT_ID: &str = "project-2c6e1dcc-3fb9-4910-adc";
const DEFAULT_VERTEX_AI_LOCATION: &str = "eu";
const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

#[derive(Clone, PartialEq, Eq)]
pub struct ApiConfig {
    bind_addr: SocketAddr,
    cognito_jwt: CognitoJwtConfig,
    vertex_ai_embedding: VertexAiEmbeddingConfig,
    google_geocoder: GoogleGeocoderConfig,
    stripe_billing: StripeBillingConfig,
    billing_prices: BillingPriceIds,
    zoho: ZohoConfig,
}

impl ApiConfig {
    pub fn from_env() -> Result<Self, ApiConfigError> {
        Self::from_getter(|name| std::env::var(name).ok())
    }

    pub fn from_getter<F>(mut get: F) -> Result<Self, ApiConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let raw_bind_addr =
            get(API_BIND_ADDR_ENV).unwrap_or_else(|| DEFAULT_API_BIND_ADDR.to_owned());
        let bind_addr =
            raw_bind_addr
                .parse()
                .map_err(|source| ApiConfigError::InvalidBindAddr {
                    value: raw_bind_addr,
                    source,
                })?;

        let issuer = required_config(&mut get, COGNITO_ISSUER_ENV)?;
        let jwks_url = required_config(&mut get, COGNITO_JWKS_URL_ENV)?;
        let app_client_ids = required_config(&mut get, COGNITO_APP_CLIENT_IDS_ENV)?
            .split(',')
            .map(str::trim)
            .filter(|client_id| !client_id.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if app_client_ids.is_empty() {
            return Err(ApiConfigError::EmptyCognitoAppClientIds);
        }
        let vertex_ai_embedding = VertexAiEmbeddingConfig::new(
            get(VERTEX_AI_PROJECT_ID_ENV)
                .unwrap_or_else(|| DEFAULT_VERTEX_AI_PROJECT_ID.to_owned()),
            get(VERTEX_AI_LOCATION_ENV).unwrap_or_else(|| DEFAULT_VERTEX_AI_LOCATION.to_owned()),
        );
        let google_geocoder = GoogleGeocoderConfig::new(
            get(GOOGLE_GEOCODING_API_KEY_ENV)
                .filter(|api_key| !api_key.trim().is_empty())
                .ok_or(ApiConfigError::MissingGoogleGeocodingApiKey)?,
        );
        let stripe_billing = StripeBillingConfig {
            api_key: required_config(&mut get, STRIPE_API_KEY_ENV)?,
            checkout_success_url: required_url_config(&mut get, STRIPE_CHECKOUT_SUCCESS_URL_ENV)?,
            checkout_cancel_url: required_url_config(&mut get, STRIPE_CHECKOUT_CANCEL_URL_ENV)?,
            portal_return_url: required_url_config(&mut get, STRIPE_PORTAL_RETURN_URL_ENV)?,
        };
        let billing_prices = BillingPriceIds {
            pro_monthly: required_config(&mut get, STRIPE_PRO_MONTHLY_PRICE_ID_ENV)?,
            pro_yearly: required_config(&mut get, STRIPE_PRO_YEARLY_PRICE_ID_ENV)?,
            ultimate_monthly: required_config(&mut get, STRIPE_ULTIMATE_MONTHLY_PRICE_ID_ENV)?,
            ultimate_yearly: required_config(&mut get, STRIPE_ULTIMATE_YEARLY_PRICE_ID_ENV)?,
        };

        let zoho = ZohoConfig {
            list_key: required_config(&mut get, ZOHO_LIST_KEY_ENV)?,
            client_id: required_config(&mut get, ZOHO_CLIENT_ID_ENV)?,
            client_secret: required_config(&mut get, ZOHO_CLIENT_SECRET_ENV)?,
            refresh_token: required_config(&mut get, ZOHO_REFRESH_TOKEN_ENV)?,
            accounts_url: required_config(&mut get, ZOHO_ACCOUNTS_URL_ENV)?,
            campaigns_url: required_config(&mut get, ZOHO_CAMPAIGNS_URL_ENV)?,
        };

        Ok(Self {
            bind_addr,
            cognito_jwt: CognitoJwtConfig::new(issuer, jwks_url, app_client_ids),
            vertex_ai_embedding,
            google_geocoder,
            stripe_billing,
            billing_prices,
            zoho,
        })
    }

    pub const fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub fn cognito_jwt(&self) -> &CognitoJwtConfig {
        &self.cognito_jwt
    }

    pub fn vertex_ai_embedding(&self) -> &VertexAiEmbeddingConfig {
        &self.vertex_ai_embedding
    }

    fn google_geocoder(&self) -> &GoogleGeocoderConfig {
        &self.google_geocoder
    }

    fn stripe_billing(&self) -> &StripeBillingConfig {
        &self.stripe_billing
    }

    fn billing_prices(&self) -> &BillingPriceIds {
        &self.billing_prices
    }

    fn zoho(&self) -> &ZohoConfig {
        &self.zoho
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ZohoConfig {
    list_key: String,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    accounts_url: String,
    campaigns_url: String,
}

fn required_config<F>(get: &mut F, name: &'static str) -> Result<String, ApiConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    get(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or(ApiConfigError::MissingRequiredConfig { name })
}

fn required_url_config<F>(get: &mut F, name: &'static str) -> Result<url::Url, ApiConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let value = required_config(get, name)?;
    url::Url::parse(&value).map_err(|source| ApiConfigError::InvalidUrlConfig { name, source })
}

#[derive(thiserror::Error, Debug)]
pub enum ApiConfigError {
    #[error("invalid {env_name}: {value}", env_name = API_BIND_ADDR_ENV)]
    InvalidBindAddr {
        value: String,
        source: AddrParseError,
    },
    #[error("missing required configuration {name}")]
    MissingRequiredConfig { name: &'static str },
    #[error("invalid URL configuration {name}")]
    InvalidUrlConfig {
        name: &'static str,
        #[source]
        source: url::ParseError,
    },
    #[error("{COGNITO_APP_CLIENT_IDS_ENV} must contain at least one client id")]
    EmptyCognitoAppClientIds,
    #[error(
        "missing required environment variable {env_name}",
        env_name = GOOGLE_GEOCODING_API_KEY_ENV
    )]
    MissingGoogleGeocodingApiKey,
}

pub fn app(state: AppState) -> Router {
    let health_routes = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .with_state(Arc::clone(&state.readiness));
    let shop_routes = Router::new()
        .route(
            "/api/v1/me/partner-shops",
            get(shops::get_partner_shops::get_partner_shops),
        )
        .route(
            "/api/v1/by-slug/shops/{shop_slug_id}",
            get(shops::get_shop_by_slug::get_shop_by_slug),
        )
        .route(
            "/api/v1/shops",
            get(shops::search_shops::get_shops).post(shops::create_shop::create_shop),
        )
        .route(
            "/api/v1/shops/{shop_id}",
            get(shops::get_shop::get_shop).patch(shops::update_shop::update_shop),
        )
        .with_state(state.shops);
    let mut routes = health_routes.merge(shop_routes);

    if let Some(products) = state.products {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/products",
                    get(products::search_products::get_products),
                )
                .route(
                    "/api/v1/products/{product_id}",
                    get(products::get_product_by_id::get_product_by_id),
                )
                .route(
                    "/api/v1/products/{product_id}/history",
                    get(products::get_product_history::get_product_events_by_id),
                )
                .route(
                    "/api/v1/products/{product_id}/similar",
                    get(products::get_similar_products::get_similar_products_by_id),
                )
                .route(
                    "/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}",
                    get(products::get_product_by_slug::get_product_by_slug),
                )
                .route(
                    "/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}/history",
                    get(products::get_product_history::get_product_events_by_slug),
                )
                .route(
                    "/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}/similar",
                    get(products::get_similar_products::get_similar_products_by_slug),
                )
                .with_state(products),
        );
    }

    if let Some(webhooks) = state.webhooks {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/webhooks/woocommerce/{shop_id}",
                    post(webhooks::post_woocommerce::post_woocommerce),
                )
                .with_state(webhooks),
        );
    }

    if let Some(partner_products) = state.partner_products {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/shops/{shop_id}/products",
                    post(partner_products::create_products::create_products)
                        .patch(partner_products::update_products::update_products)
                        .put(partner_products::upsert_products::upsert_products)
                        .delete(partner_products::delete_products::delete_products),
                )
                .with_state(partner_products),
        );
    }

    if let Some(oauth) = state.oauth {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/oauth/clients",
                    get(oauth::list_clients::list_clients)
                        .post(oauth::create_client::create_client),
                )
                .route(
                    "/api/v1/oauth/clients/{client_id}",
                    get(oauth::get_client::get_client)
                        .patch(oauth::update_client::update_client)
                        .delete(oauth::delete_client::delete_client),
                )
                .route("/api/v1/oauth/authorize", get(oauth::authorize::authorize))
                .route("/api/v1/oauth/token", post(oauth::token::token))
                .route(
                    "/api/v1/oauth/tokens/by-third-party-code/{third_party_code}",
                    get(oauth::token_by_third_party_code::token_by_third_party_code),
                )
                .route("/api/v1/oauth/revoke", post(oauth::revoke::revoke))
                .route(
                    "/api/v1/oauth/introspect",
                    post(oauth::introspect::introspect),
                )
                .with_state(oauth),
        );
    }

    if let Some(users) = state.users {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/me/account",
                    get(users::account::get_me).patch(users::account::patch_me),
                )
                .route("/api/v1/me", delete(users::account::delete_me))
                .route(
                    "/api/v1/me/access-tokens",
                    get(users::access_tokens::list_access_tokens)
                        .post(users::access_tokens::post_access_token)
                        .patch(users::access_tokens::patch_access_token),
                )
                .route(
                    "/api/v1/me/access-tokens/{access_token_id}",
                    get(users::access_tokens::get_access_token)
                        .delete(users::access_tokens::delete_access_token),
                )
                .route("/api/v1/users", get(users::admin_users::search_users))
                .route(
                    "/api/v1/users/{user_id}",
                    get(users::admin_users::get_user)
                        .patch(users::admin_users::patch_admin_user)
                        .delete(users::admin_users::delete_admin_user),
                )
                .with_state(users),
        );
    }

    if let Some(watchlist) = state.watchlist {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/me/watchlist",
                    get(watchlist::list::list_watchlist).post(watchlist::create::post_watchlist),
                )
                .route(
                    "/api/v1/me/watchlist/{product_id}",
                    patch(watchlist::update::patch_watchlist)
                        .delete(watchlist::delete::delete_watchlist),
                )
                .with_state(watchlist),
        );
    }

    if let Some(search_filters) = state.search_filters {
        routes = routes.merge(search_filters::router(search_filters));
    }

    if let Some(billing) = state.billing {
        routes = routes.merge(
            Router::new()
                .route("/api/v1/me/billing/checkout", post(billing::checkout))
                .route("/api/v1/me/billing/portal", post(billing::portal))
                .route("/api/v1/me/billing/manage", post(billing::manage))
                .with_state(billing),
        );
    }

    if let Some(newsletter) = state.newsletter {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/newsletter-subscriptions",
                    axum::routing::put(newsletter::put::put_newsletter_subscription),
                )
                .with_state(newsletter),
        );
    }

    if let Some(partner_applications) = state.partner_applications {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/me/partner-applications",
                    get(partner_applications::personal::list_me)
                        .post(partner_applications::personal::post_me),
                )
                .route(
                    "/api/v1/me/partner-applications/{partner_application_id}",
                    get(partner_applications::personal::get_me)
                        .delete(partner_applications::personal::delete_me),
                )
                .route(
                    "/api/v1/partner-applications",
                    get(partner_applications::admin::admin_list),
                )
                .route(
                    "/api/v1/partner-applications/{partner_application_id}",
                    get(partner_applications::admin::admin_get)
                        .patch(partner_applications::admin::admin_patch),
                )
                .route(
                    "/api/v1/partner-applications/{partner_application_id}/decision",
                    post(partner_applications::admin::admin_decision),
                )
                .with_state(partner_applications),
        );
    }

    with_transport_middleware(routes)
}

async fn health() -> &'static str {
    "ok\n"
}

async fn ready(
    axum::extract::State(readiness): axum::extract::State<Arc<dyn ReadinessCheck>>,
) -> axum::http::StatusCode {
    match readiness.check().await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT,
        Err(()) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    }
}

struct RuntimeReadiness {
    postgres: PgPool,
    dynamodb: aws_sdk_dynamodb::Client,
    dynamodb_table_name: String,
    opensearch: OpenSearch,
}

#[async_trait::async_trait]
impl ReadinessCheck for RuntimeReadiness {
    async fn check(&self) -> Result<(), ()> {
        self.postgres.acquire().await.map_err(|_| ())?;
        self.dynamodb
            .describe_table()
            .table_name(&self.dynamodb_table_name)
            .send()
            .await
            .map_err(|_| ())?;
        self.opensearch.ping().send().await.map_err(|_| ())?;
        Ok(())
    }
}

pub async fn app_state_from_env() -> Result<AppState, ApiStateError> {
    let config = ApiConfig::from_env().map_err(ApiStateError::Config)?;
    app_state_from_config(&config).await
}

async fn app_state_from_config(config: &ApiConfig) -> Result<AppState, ApiStateError> {
    let pool = common::postgres::connect_from_env().await?;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let get_product_events =
        GetProductEventsHandler::new(unit_of_work.clone(), SqlxProductEventReaderFactory::new());
    let search_filter_reader = SqlxSearchFilterReader::new(pool.clone());
    let opensearch_client = opensearch_client_from_env()?;
    let embeddings: Arc<dyn EmbeddingGenerator> = Arc::new(VertexAiEmbeddingGenerator::new(
        config.vertex_ai_embedding().clone(),
        google_application_default_credentials()?,
    ));

    let get_shop = GetShopHandler::new(unit_of_work.clone(), SqlxShopDetailsReaderFactory::new());
    let search_shops =
        SearchShopsHandler::new(unit_of_work.clone(), SqlxShopSearchReaderFactory::new());
    let check_user_admin =
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new());
    let geocoder = Arc::new(GoogleGeocoder::new(config.google_geocoder().clone()));
    let create_shop = CreateShopHandler::new(
        unit_of_work.clone(),
        SqlxShopRepositoryFactory::new(),
        Arc::clone(&geocoder),
        check_user_admin,
    );
    let update_shop = UpdateShopHandler::new(
        unit_of_work.clone(),
        SqlxShopRepositoryFactory::new(),
        Arc::clone(&geocoder),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        SqlxPartnerShopReaderFactory::new(),
    );
    let list_user_partner_shops =
        ListUserPartnerShopsHandler::new(unit_of_work.clone(), SqlxPartnerShopReaderFactory::new());
    let get_own_user =
        GetOwnUserHandler::new(unit_of_work.clone(), SqlxUserAccountReaderFactory::new());
    let stripe_billing = StripeBillingClient::new(config.stripe_billing().clone())
        .map_err(ApiStateError::StripeBilling)?;
    let billing_prices = config.billing_prices().clone();
    let admin_get_user = AdminGetUserHandler::new(
        unit_of_work.clone(),
        SqlxUserAccountReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let search_users = SearchUsersHandler::new(
        unit_of_work.clone(),
        SqlxUserSearchReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let update_user_profile = UpdateUserProfileHandler::new(
        unit_of_work.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let change_user_role = ChangeUserRoleHandler::new(
        unit_of_work.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let change_user_tier = ChangeUserTierHandler::new(
        unit_of_work.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
        SqlxUserTierEntitlementsFactory::new(),
    );
    let delete_user = DeleteUserHandler::new(
        unit_of_work.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let newsletter_writer = ZohoNewsletterSubscriptionWriter::new(
        config.zoho().list_key.clone(),
        reqwest::Client::new(),
        config.zoho().client_id.clone(),
        config.zoho().client_secret.clone(),
        config.zoho().refresh_token.clone(),
        config.zoho().accounts_url.clone(),
        config.zoho().campaigns_url.clone(),
    );
    let upsert_newsletter_subscription = UpsertNewsletterSubscriptionHandler::new(
        SqlxNewsletterProfileReader::new(pool.clone()),
        newsletter_writer,
    );
    let watch_product = WatchProductHandler::new(
        unit_of_work.clone(),
        SqlxWatchlistRepositoryFactory,
        SqlxWatchlistQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory::new(),
    );
    let update_watchlist_product = UpdateWatchlistProductHandler::new(
        unit_of_work.clone(),
        SqlxWatchlistRepositoryFactory,
        SqlxWatchlistQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory::new(),
    );
    let unwatch_product =
        UnwatchProductHandler::new(unit_of_work.clone(), SqlxWatchlistRepositoryFactory);
    let create_partner_application = CreatePartnerShopApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
        SqlxShopRepositoryFactory::new(),
        geocoder,
    );
    let list_partner_applications = ListPartnerShopApplicationsHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationReaderFactory::new(),
    );
    let get_partner_application = GetPartnerShopApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
    );
    let delete_partner_application = WithdrawPartnerShopApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
        SqlxShopRepositoryFactory::new(),
    );
    let admin_list_partner_applications = AdminListPartnerShopApplicationsHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let admin_get_partner_application = AdminGetPartnerShopApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let admin_update_partner_application = AdminUpdatePartnerShopApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let dynamodb_client = Box::leak(Box::new(aws_sdk_dynamodb::Client::new(&aws_config)));
    let table_name =
        std::env::var(DYNAMODB_TABLE_NAME_ENV).map_err(|_| ApiStateError::MissingEnv {
            name: DYNAMODB_TABLE_NAME_ENV,
        })?;
    let readiness_table_name = table_name.clone();
    let table_name = Box::leak(table_name.into_boxed_str());
    let table_name_ref: &str = table_name;
    let admin_decide_partner_application = AdminDecidePartnerShopApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
        SqlxShopRepositoryFactory::new(),
        SqlxUserPartnerShopMembershipRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
        CreateNotificationHandler::new(ConditionalDynamoDbNotificationWriter::new(
            (*dynamodb_client).clone(),
            table_name_ref,
        )),
    );
    let product_user_states = SqlxProductUserStateReader::new(pool.clone());
    let get_similar_products = GetSimilarProductsHandler::new(
        unit_of_work.clone(),
        SqlxProductEmbeddingReaderFactory::new(),
        OpenSearchProductSimilarProductsReader::new(opensearch_client.clone()),
        product_user_states.clone(),
        DynamoDbAllNotificationsReader::new(dynamodb_client, table_name_ref),
    );
    let search_products = SearchProductsHandler::new(
        OpenSearchProductSearchReader::new(opensearch_client.clone()),
        Arc::clone(&embeddings),
        product_user_states,
        DynamoDbAllNotificationsReader::new(dynamodb_client, table_name_ref),
    );
    let get_product = GetProductHandler::new(
        unit_of_work.clone(),
        SqlxProductDetailsReaderFactory::new(),
        DynamoDbProductNotificationsReader::new(dynamodb_client, table_name_ref),
    );
    let create_product = CreateProductHandler::new_with_fx_rates(
        unit_of_work.clone(),
        SqlxProductRepositoryFactory::new(),
        SqlxProductEventStoreFactory::new(),
        SqlxPartnerProductAuthorizerFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
    );
    let update_product = UpdateProductHandler::new_with_fx_rates(
        unit_of_work.clone(),
        SqlxProductRepositoryFactory::new(),
        SqlxProductEventStoreFactory::new(),
        SqlxPartnerProductAuthorizerFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
    );
    let upsert_product = UpsertProductHandler::new_with_fx_rates(
        unit_of_work.clone(),
        SqlxProductRepositoryFactory::new(),
        SqlxProductEventStoreFactory::new(),
        SqlxPartnerProductAuthorizerFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
    );
    let delete_product = DeleteProductHandler::new(
        unit_of_work.clone(),
        SqlxProductRepositoryFactory::new(),
        SqlxProductEventStoreFactory::new(),
        SqlxPartnerProductAuthorizerFactory::new(),
    );
    let ingest_woocommerce_product = IngestWoocommerceProductHandler::new_with_fx_rates(
        unit_of_work.clone(),
        SqlxPartnerShopReaderFactory::new(),
        SqlxWoocommerceWebhookShopReaderFactory::new(),
        SqlxWoocommerceWebhookSignatureVerifierFactory::new(),
        SqlxProductRepositoryFactory::new(),
        SqlxProductEventStoreFactory::new(),
        SqlxPartnerProductAuthorizerFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
    );
    let list_watchlist = ListWatchlistHandler::new(
        unit_of_work.clone(),
        SqlxProductWatchlistDetailsReaderFactory::new(),
        DynamoDbAllNotificationsReader::new(dynamodb_client, table_name_ref),
    );
    let access_token_store = DynamoDbAccessTokenStore::new(dynamodb_client, table_name_ref);
    let oauth_store = OAuthDynamoDbStore::new(dynamodb_client, table_name_ref);
    let oauth_access_tokens = StoreOAuthAccessTokenGateway::new(access_token_store.clone());
    let access_token_use_case = AuthenticateAccessTokenHandler::new(access_token_store.clone());
    let jwks_client = reqwest::Client::builder()
        .connect_timeout(JWKS_CONNECT_TIMEOUT)
        .timeout(JWKS_REQUEST_TIMEOUT)
        .build()
        .map_err(ApiStateError::JwksClient)?;
    let authenticator = compose_authenticator(
        config,
        ReqwestJwksProvider::new(jwks_client),
        AuraAccessTokenAuthenticator::new(access_token_use_case),
    )
    .map_err(ApiStateError::CognitoJwt)?;
    let billing_state = BillingState::new(
        Arc::new(CreateBillingCheckoutSessionHandler::new(
            GetOwnUserHandler::new(unit_of_work.clone(), SqlxUserAccountReaderFactory::new()),
            AssociateUserStripeCustomerIdHandler::new(
                unit_of_work.clone(),
                SqlxUserRepositoryFactory::new(),
            ),
            stripe_billing.clone(),
            stripe_billing.clone(),
            billing_prices.clone(),
        )),
        Arc::new(CreateBillingPortalSessionHandler::new(
            GetOwnUserHandler::new(unit_of_work.clone(), SqlxUserAccountReaderFactory::new()),
            stripe_billing.clone(),
        )),
        Arc::new(CreateBillingManagementSessionHandler::new(
            GetOwnUserHandler::new(unit_of_work.clone(), SqlxUserAccountReaderFactory::new()),
            AssociateUserStripeCustomerIdHandler::new(
                unit_of_work.clone(),
                SqlxUserRepositoryFactory::new(),
            ),
            stripe_billing.clone(),
            stripe_billing.clone(),
            stripe_billing,
            billing_prices,
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let partner_products_state = PartnerProductsState::new(
        Arc::new(create_product),
        Arc::new(update_product),
        Arc::new(upsert_product),
        Arc::new(delete_product),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let users_state = UsersState {
        get_own_user: Arc::new(get_own_user),
        admin_get_user: Arc::new(admin_get_user),
        search_users: Arc::new(search_users),
        update_user_profile: Arc::new(update_user_profile),
        change_user_role: Arc::new(change_user_role),
        change_user_tier: Arc::new(change_user_tier),
        delete_user: Arc::new(delete_user),
        create_access_token: Arc::new(CreateAccessTokenHandler::new(access_token_store.clone())),
        list_access_tokens: Arc::new(ListAccessTokensHandler::new(access_token_store.clone())),
        get_access_token: Arc::new(GetAccessTokenHandler::new(access_token_store.clone())),
        update_access_token: Arc::new(UpdateAccessTokenHandler::new(access_token_store.clone())),
        delete_access_token: Arc::new(DeleteAccessTokenHandler::new(access_token_store)),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    let search_filters_state = SearchFiltersState::new(
        Arc::new(ListOwnedSearchFiltersHandler::new(
            search_filter_reader.clone(),
        )),
        Arc::new(CreateSearchFilterHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
            Arc::clone(&embeddings),
            SqlxSearchFilterQuotaReaderFactory,
            SqlxUserTierEntitlementsFactory::new(),
        )),
        Arc::new(GetOwnedSearchFilterHandler::new(
            search_filter_reader.clone(),
        )),
        Arc::new(UpdateOwnedSearchFilterHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
            Arc::clone(&embeddings),
            search_filter_reader.clone(),
            SqlxSearchFilterQuotaReaderFactory,
            SqlxUserTierEntitlementsFactory::new(),
        )),
        Arc::new(DeleteOwnedSearchFilterHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
        )),
        Arc::new(ListSearchFilterMatchesHandler::new(
            search_filter_reader.clone(),
            SqlxProductDetailsBatchReader::new(pool.clone()),
            DynamoDbAllNotificationsReader::new(dynamodb_client, table_name_ref),
        )),
        Arc::new(UpdateSearchFilterMatchFeedbackHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
            SqlxSearchFilterMatchRepositoryFactory,
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let watchlist_state = WatchlistState {
        list_watchlist: Arc::new(list_watchlist),
        watch_product: Arc::new(watch_product),
        update_watchlist_product: Arc::new(update_watchlist_product),
        unwatch_product: Arc::new(unwatch_product),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    let oauth_state = OAuthState {
        create_client: Arc::new(CreateOAuthClientHandler::new(oauth_store.clone())),
        list_clients: Arc::new(ListOAuthClientsHandler::new(oauth_store.clone())),
        get_client: Arc::new(GetOAuthClientHandler::new(oauth_store.clone())),
        update_client: Arc::new(UpdateOAuthClientHandler::new(oauth_store.clone())),
        delete_client: Arc::new(DeleteOAuthClientHandler::new(oauth_store.clone())),
        authorize: Arc::new(AuthorizeHandler::new(
            oauth_store.clone(),
            oauth_store.clone(),
        )),
        token_by_authorization_code: Arc::new(TokenByAuthorizationCodeHandler::new(
            oauth_store.clone(),
            oauth_store.clone(),
            oauth_store.clone(),
            oauth_access_tokens.clone(),
        )),
        token_by_third_party_code: Arc::new(TokenByThirdPartyCodeHandler::new(oauth_store.clone())),
        revoke: Arc::new(RevokeTokenHandler::new(
            oauth_store.clone(),
            oauth_access_tokens.clone(),
        )),
        introspect: Arc::new(IntrospectTokenHandler::new(
            oauth_store,
            oauth_access_tokens,
        )),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    let partner_state = PartnerApplicationsState {
        create: Arc::new(create_partner_application),
        list: Arc::new(list_partner_applications),
        get: Arc::new(get_partner_application),
        delete: Arc::new(delete_partner_application),
        admin_list: Arc::new(admin_list_partner_applications),
        admin_get: Arc::new(admin_get_partner_application),
        admin_update: Arc::new(admin_update_partner_application),
        admin_decide: Arc::new(admin_decide_partner_application),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };

    let readiness = Arc::new(RuntimeReadiness {
        postgres: pool,
        dynamodb: dynamodb_client.clone(),
        dynamodb_table_name: readiness_table_name,
        opensearch: opensearch_client.clone(),
    });

    Ok(AppState::new(
        ShopsState::new(
            Arc::new(get_shop),
            Arc::new(search_shops),
            Arc::new(create_shop),
            Arc::new(update_shop),
            Arc::new(list_user_partner_shops),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ),
        users_state,
        watchlist_state,
        partner_state,
    )
    .with_products(
        ProductsState::new(
            Arc::new(get_product),
            Arc::new(get_similar_products),
            Arc::new(search_products),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        )
        .with_product_events(Arc::new(get_product_events)),
    )
    .with_partner_products(partner_products_state)
    .with_webhooks(WebhooksState::new(
        Arc::new(ingest_woocommerce_product),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    ))
    .with_oauth(oauth_state)
    .with_search_filters(search_filters_state)
    .with_billing(billing_state)
    .with_newsletter(NewsletterState::new(
        Arc::new(upsert_newsletter_subscription),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    ))
    .with_readiness(readiness))
}

fn opensearch_client_from_env() -> Result<OpenSearch, ApiStateError> {
    let endpoint =
        std::env::var("OPENSEARCH_ENDPOINT_URL").map_err(|_| ApiStateError::MissingEnv {
            name: "OPENSEARCH_ENDPOINT_URL",
        })?;
    let endpoint = url::Url::parse(&endpoint).map_err(|error| ApiStateError::OpenSearch {
        detail: error.to_string(),
    })?;
    let stage = std::env::var("STAGE").unwrap_or_else(|_| "prod".to_owned());
    let transport = if stage == "ephemeral" {
        TransportBuilder::new(SingleNodeConnectionPool::new(endpoint)).build()
    } else {
        let username =
            std::env::var("OPENSEARCH_USERNAME").map_err(|_| ApiStateError::MissingEnv {
                name: "OPENSEARCH_USERNAME",
            })?;
        let password =
            std::env::var("OPENSEARCH_PASSWORD").map_err(|_| ApiStateError::MissingEnv {
                name: "OPENSEARCH_PASSWORD",
            })?;
        TransportBuilder::new(SingleNodeConnectionPool::new(endpoint))
            .auth(Credentials::Basic(username, password))
            .build()
    }
    .map_err(|error| ApiStateError::OpenSearch {
        detail: error.to_string(),
    })?;
    Ok(OpenSearch::new(transport))
}

fn google_application_default_credentials()
-> Result<google_cloud_auth::credentials::AccessTokenCredentials, ApiStateError> {
    GoogleCredentialsBuilder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|error| ApiStateError::VertexAiCredentials {
            detail: error.to_string(),
        })
}

fn compose_authenticator<P, A>(
    config: &ApiConfig,
    jwks_provider: P,
    access_token_authenticator: A,
) -> Result<Arc<dyn TokenAuthenticator>, AuthError>
where
    P: JwksProvider + 'static,
    A: TokenAuthenticator + 'static,
{
    let cognito_jwt = CognitoJwtAuthenticator::new(config.cognito_jwt().clone(), jwks_provider)?;
    Ok(Arc::new(ApiAuthService::new(
        cognito_jwt,
        access_token_authenticator,
    )))
}

#[derive(thiserror::Error, Debug)]
pub enum ApiStateError {
    #[error("failed to initialize Stripe billing client")]
    StripeBilling(#[source] billing_stripe::StripeBillingClientInitError),
    #[error(transparent)]
    Postgres(#[from] PostgresConnectError),
    #[error(transparent)]
    Config(#[from] ApiConfigError),
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("failed to configure OpenSearch: {detail}")]
    OpenSearch { detail: String },
    #[error("failed to initialize Vertex AI credentials: {detail}")]
    VertexAiCredentials { detail: String },
    #[error("failed to configure Cognito JWT authentication: {0}")]
    CognitoJwt(AuthError),
    #[error("failed to build JWKS HTTP client: {0}")]
    JwksClient(reqwest::Error),
}

pub async fn run_until_shutdown<S>(config: ApiConfig, shutdown: S) -> Result<(), ApiRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    let state = app_state_from_config(&config)
        .await
        .map_err(ApiRunError::State)?;
    let listener = TcpListener::bind(config.bind_addr())
        .await
        .map_err(ApiRunError::Bind)?;
    serve(listener, app(state), shutdown).await
}

pub async fn serve<S>(listener: TcpListener, app: Router, shutdown: S) -> Result<(), ApiRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    let local_addr = listener.local_addr().map_err(ApiRunError::LocalAddr)?;
    info!(bind_addr = %local_addr, "aura-historia-api listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(ApiRunError::Serve)
}

#[derive(thiserror::Error, Debug)]
pub enum ApiRunError {
    #[error("failed to build API state")]
    State(#[source] ApiStateError),
    #[error("failed to bind API listener")]
    Bind(#[source] std::io::Error),
    #[error("failed to read API listener local address")]
    LocalAddr(#[source] std::io::Error),
    #[error("failed to serve API")]
    Serve(#[source] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::auth::{
        AuthMethod, JsonWebKey, JsonWebKeySet, JwksProvider, RequestMetadata, TransportPrincipal,
    };
    use crate::state::ReadinessCheck;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use common::operation_context::CredentialCapability;
    use common::user_id::UserId;
    use http::StatusCode;
    use jsonwebtokens::{Algorithm, AlgorithmID};
    use openssl::rsa::Rsa;
    use serde_json::json;

    use std::collections::BTreeSet;
    use time::OffsetDateTime;
    use tokio::sync::oneshot;

    fn env(values: &[(&'static str, &str)]) -> HashMap<&'static str, String> {
        let mut environment = HashMap::from([
            (COGNITO_ISSUER_ENV, "https://issuer.example/pool".to_owned()),
            (
                COGNITO_JWKS_URL_ENV,
                "https://issuer.example/pool/.well-known/jwks.json".to_owned(),
            ),
            (COGNITO_APP_CLIENT_IDS_ENV, "audience-1".to_owned()),
            (GOOGLE_GEOCODING_API_KEY_ENV, "api-key".to_owned()),
            (STRIPE_API_KEY_ENV, "stripe-api-key".to_owned()),
            (
                STRIPE_CHECKOUT_SUCCESS_URL_ENV,
                "https://app.example/billing/success".to_owned(),
            ),
            (
                STRIPE_CHECKOUT_CANCEL_URL_ENV,
                "https://app.example/billing/cancel".to_owned(),
            ),
            (
                STRIPE_PORTAL_RETURN_URL_ENV,
                "https://app.example/billing".to_owned(),
            ),
            (
                STRIPE_PRO_MONTHLY_PRICE_ID_ENV,
                "price_pro_monthly".to_owned(),
            ),
            (
                STRIPE_PRO_YEARLY_PRICE_ID_ENV,
                "price_pro_yearly".to_owned(),
            ),
            (
                STRIPE_ULTIMATE_MONTHLY_PRICE_ID_ENV,
                "price_ultimate_monthly".to_owned(),
            ),
            (
                STRIPE_ULTIMATE_YEARLY_PRICE_ID_ENV,
                "price_ultimate_yearly".to_owned(),
            ),
            (ZOHO_LIST_KEY_ENV, "zoho-list-key".to_owned()),
            (ZOHO_CLIENT_ID_ENV, "zoho-client-id".to_owned()),
            (ZOHO_CLIENT_SECRET_ENV, "zoho-client-secret".to_owned()),
            (ZOHO_REFRESH_TOKEN_ENV, "zoho-refresh-token".to_owned()),
            (
                ZOHO_ACCOUNTS_URL_ENV,
                "https://accounts.zoho.example".to_owned(),
            ),
            (
                ZOHO_CAMPAIGNS_URL_ENV,
                "https://campaigns.zoho.example".to_owned(),
            ),
        ]);
        environment.extend(
            values
                .iter()
                .map(|(key, value)| (*key, (*value).to_owned())),
        );
        environment
    }

    #[test]
    fn should_use_default_bind_addr_when_env_missing() -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[(GOOGLE_GEOCODING_API_KEY_ENV, "api-key")]);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!("0.0.0.0:8080".parse::<SocketAddr>()?, config.bind_addr());
        Ok(())
    }

    #[test]
    fn should_read_bind_addr_from_env() -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[
            (API_BIND_ADDR_ENV, "127.0.0.1:9000"),
            (GOOGLE_GEOCODING_API_KEY_ENV, "api-key"),
        ]);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!("127.0.0.1:9000".parse::<SocketAddr>()?, config.bind_addr());
        Ok(())
    }

    #[test]
    fn should_read_vertex_ai_embedding_config_from_environment()
    -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[
            (VERTEX_AI_PROJECT_ID_ENV, "embedding-project"),
            (VERTEX_AI_LOCATION_ENV, "europe-west3"),
            (GOOGLE_GEOCODING_API_KEY_ENV, "api-key"),
        ]);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!(
            "embedding-project",
            config.vertex_ai_embedding().project_id()
        );
        assert_eq!("europe-west3", config.vertex_ai_embedding().location());
        Ok(())
    }

    #[test]
    fn should_read_cognito_config_from_environment() -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[
            (
                COGNITO_ISSUER_ENV,
                "https://cognito-idp.eu-west-1.amazonaws.com/pool",
            ),
            (
                COGNITO_JWKS_URL_ENV,
                "https://cognito-idp.eu-west-1.amazonaws.com/pool/.well-known/jwks.json",
            ),
            (COGNITO_APP_CLIENT_IDS_ENV, "client-1, client-2"),
        ]);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!(
            "https://cognito-idp.eu-west-1.amazonaws.com/pool",
            config.cognito_jwt().issuer
        );
        assert_eq!(
            "https://cognito-idp.eu-west-1.amazonaws.com/pool/.well-known/jwks.json",
            config.cognito_jwt().jwks_url
        );
        assert_eq!(
            std::collections::HashSet::from(["client-1".to_owned(), "client-2".to_owned()]),
            config.cognito_jwt().app_client_ids
        );
        Ok(())
    }

    #[test]
    fn should_fail_when_cognito_config_missing() {
        let values = HashMap::<&'static str, String>::new();

        let config = ApiConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(
            config,
            Err(ApiConfigError::MissingRequiredConfig {
                name: COGNITO_ISSUER_ENV
            })
        ));
    }

    #[test]
    fn should_fail_when_google_geocoding_api_key_is_missing() {
        let mut values = env(&[]);
        values.remove(GOOGLE_GEOCODING_API_KEY_ENV);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(
            config,
            Err(ApiConfigError::MissingGoogleGeocodingApiKey)
        ));
    }

    #[test]
    fn should_read_google_geocoding_api_key_from_environment()
    -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[(GOOGLE_GEOCODING_API_KEY_ENV, "configured-api-key")]);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned())?;

        assert!(config.google_geocoder() == &GoogleGeocoderConfig::new("configured-api-key"));
        Ok(())
    }

    #[test]
    fn should_fail_when_bind_addr_is_invalid() {
        let values = env(&[(API_BIND_ADDR_ENV, "not-an-addr")]);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(
            config,
            Err(ApiConfigError::InvalidBindAddr { .. })
        ));
    }

    #[derive(Clone)]
    struct StaticJwksProvider {
        jwks: JsonWebKeySet,
    }

    #[async_trait::async_trait]
    impl JwksProvider for StaticJwksProvider {
        async fn fetch_jwks(&self, _jwks_url: &str) -> Result<JsonWebKeySet, AuthError> {
            Ok(self.jwks.clone())
        }
    }

    #[tokio::test]
    async fn should_authenticate_cognito_and_aura_tokens_from_composed_authenticator()
    -> Result<(), Box<dyn std::error::Error>> {
        let rsa = Rsa::generate(2048)?;
        let private_pem = rsa.private_key_to_pem()?;
        let algorithm = Algorithm::new_rsa_pem_signer(AlgorithmID::RS256, &private_pem)?;
        let user_id = UserId::new();
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let token = jsonwebtokens::encode(
            &json!({ "alg": algorithm.name(), "kid": "kid-1" }),
            &json!({
                "iss": "https://issuer.example/pool",
                "sub": user_id.to_string(),
                "token_use": "access",
                "client_id": "audience-1",
                "iat": now,
                "exp": now + 3_600,
            }),
            &algorithm,
        )?;
        let config_values = env(&[]);
        let config = ApiConfig::from_getter(|name| config_values.get(name).cloned())?;
        let authenticator = compose_authenticator(
            &config,
            StaticJwksProvider {
                jwks: JsonWebKeySet {
                    keys: vec![JsonWebKey {
                        kid: "kid-1".to_owned(),
                        alg: Some("RS256".to_owned()),
                        n: URL_SAFE_NO_PAD.encode(rsa.n().to_vec()),
                        e: URL_SAFE_NO_PAD.encode(rsa.e().to_vec()),
                    }],
                },
            },
            StaticAuthenticator,
        )?;

        let cognito_principal = authenticator
            .authenticate(&token, &RequestMetadata::new("req-1", "corr-1"))
            .await?;
        let aura_principal = authenticator
            .authenticate(
                "aurahistoria_accesstoken_short_long",
                &RequestMetadata::new("req-2", "corr-2"),
            )
            .await?;

        assert!(matches!(
            cognito_principal,
            TransportPrincipal::User {
                user_id: actual,
                auth_method: AuthMethod::CognitoJwt,
                ..
            } if actual == user_id
        ));
        assert!(matches!(
            aura_principal,
            TransportPrincipal::User {
                auth_method: AuthMethod::AuraAccessToken,
                ..
            }
        ));
        Ok(())
    }

    #[derive(Clone, Copy)]
    struct Unready;

    #[async_trait::async_trait]
    impl ReadinessCheck for Unready {
        async fn check(&self) -> Result<(), ()> {
            Err(())
        }
    }

    #[tokio::test]
    async fn should_return_service_unavailable_when_dependency_readiness_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app(test_state().with_readiness(Arc::new(Unready)))
            .oneshot(http::Request::get("/ready").body(axum::body::Body::empty())?)
            .await?;

        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
        Ok(())
    }

    #[tokio::test]
    async fn should_route_health_endpoints() -> Result<(), Box<dyn std::error::Error>> {
        for (path, status_code, body) in [
            ("/health", StatusCode::OK, "ok\n"),
            ("/ready", StatusCode::NO_CONTENT, ""),
        ] {
            let response = app(test_state())
                .oneshot(http::Request::get(path).body(axum::body::Body::empty())?)
                .await?;

            assert_eq!(status_code, response.status());
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
            assert_eq!(body.as_bytes(), bytes.as_ref());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_serve_health_endpoint_until_shutdown() -> Result<(), Box<dyn std::error::Error>>
    {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve(listener, app(test_state()), async move {
            let _ = shutdown_rx.await;
        }));

        let response = reqwest::get(format!("http://{addr}/health")).await?;
        let _send_result = shutdown_tx.send(());
        server.await??;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!("ok\n", response.text().await?);
        Ok(())
    }

    fn test_state() -> AppState {
        let authenticator = Arc::new(StaticAuthenticator);
        AppState::new(
            ShopsState::new(
                Arc::new(RejectGetShopUseCase),
                Arc::new(UnusedUseCase),
                Arc::new(UnusedUseCase),
                Arc::new(UnusedUseCase),
                Arc::new(UnusedUseCase),
                authenticator.clone(),
            ),
            UsersState {
                get_own_user: Arc::new(UnusedUseCase),
                admin_get_user: Arc::new(UnusedUseCase),
                search_users: Arc::new(UnusedUseCase),
                update_user_profile: Arc::new(UnusedUseCase),
                change_user_role: Arc::new(UnusedUseCase),
                change_user_tier: Arc::new(UnusedUseCase),
                delete_user: Arc::new(UnusedUseCase),
                create_access_token: Arc::new(UnusedUseCase),
                list_access_tokens: Arc::new(UnusedUseCase),
                get_access_token: Arc::new(UnusedUseCase),
                update_access_token: Arc::new(UnusedUseCase),
                delete_access_token: Arc::new(UnusedUseCase),
                authenticator: authenticator.clone(),
            },
            WatchlistState {
                list_watchlist: Arc::new(UnusedUseCase),
                watch_product: Arc::new(UnusedUseCase),
                update_watchlist_product: Arc::new(UnusedUseCase),
                unwatch_product: Arc::new(UnusedUseCase),
                authenticator: authenticator.clone(),
            },
            PartnerApplicationsState {
                create: Arc::new(UnusedUseCase),
                list: Arc::new(UnusedUseCase),
                get: Arc::new(UnusedUseCase),
                delete: Arc::new(UnusedUseCase),
                admin_list: Arc::new(UnusedUseCase),
                admin_get: Arc::new(UnusedUseCase),
                admin_update: Arc::new(UnusedUseCase),
                admin_decide: Arc::new(UnusedUseCase),
                authenticator,
            },
        )
    }

    struct RejectGetShopUseCase;

    #[async_trait::async_trait]
    impl shop_service::use_cases::queries::get_shop::GetShopUseCase for RejectGetShopUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: shop_service::use_cases::queries::get_shop::GetShopRequest,
        ) -> Result<
            shop_service::use_cases::queries::get_shop::ShopDetailsView,
            shop_service::use_cases::queries::get_shop::GetShopError,
        > {
            Err(shop_service::use_cases::queries::get_shop::GetShopError::NotFound)
        }
    }

    struct UnusedUseCase;

    #[async_trait::async_trait]
    impl shop_service::use_cases::queries::search_shops::SearchShopsUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: shop_service::use_cases::queries::search_shops::SearchShopsRequest,
        ) -> Result<
            shop_service::use_cases::queries::search_shops::SearchShopsResult,
            shop_service::use_cases::queries::search_shops::SearchShopsError,
        > {
            unreachable!("unused search use case")
        }
    }

    #[async_trait::async_trait]
    impl shop_service::use_cases::commands::create_shop::CreateShopUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: shop_service::use_cases::commands::create_shop::CreateShopCommand,
        ) -> Result<
            shop_service::use_cases::commands::create_shop::CreateShopResult,
            shop_service::use_cases::commands::create_shop::CreateShopError,
        > {
            unreachable!("unused create shop use case")
        }
    }

    #[async_trait::async_trait]
    impl shop_service::use_cases::commands::update_shop::UpdateShopUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: shop_service::use_cases::commands::update_shop::UpdateShopCommand,
        ) -> Result<
            shop_service::use_cases::commands::update_shop::UpdateShopResult,
            shop_service::use_cases::commands::update_shop::UpdateShopError,
        > {
            unreachable!("unused update shop use case")
        }
    }

    #[async_trait::async_trait]
    impl shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsUseCase
        for UnusedUseCase
    {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsRequest,
        ) -> Result<
            shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsResult,
            shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsError,
        > {
            unreachable!("unused partner shops use case")
        }
    }

    #[async_trait::async_trait]
    impl user_service::use_cases::queries::get_own_user::GetOwnUserUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: user_service::use_cases::queries::get_own_user::GetOwnUserRequest,
        ) -> Result<
            user_service::ports::UserDetailsView,
            user_service::use_cases::queries::get_own_user::GetOwnUserError,
        > {
            unreachable!("unused get own user")
        }
    }

    #[async_trait::async_trait]
    impl user_service::use_cases::queries::admin_get_user::AdminGetUserUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: user_service::use_cases::queries::admin_get_user::AdminGetUserRequest,
        ) -> Result<
            user_service::ports::UserDetailsView,
            user_service::use_cases::queries::admin_get_user::AdminGetUserError,
        > {
            unreachable!("unused admin get user")
        }
    }
    #[async_trait::async_trait]
    impl user_service::use_cases::queries::search_users::SearchUsersUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: user_service::use_cases::queries::search_users::SearchUsersRequest,
        ) -> Result<
            user_service::use_cases::queries::search_users::SearchUsersResult,
            user_service::use_cases::queries::search_users::SearchUsersError,
        > {
            unreachable!("unused search users")
        }
    }
    #[async_trait::async_trait]
    impl user_service::use_cases::commands::update_user_profile::UpdateUserProfileUseCase
        for UnusedUseCase
    {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: user_service::use_cases::commands::update_user_profile::UpdateUserProfileCommand,
        ) -> Result<
            user_service::use_cases::commands::update_user_profile::UpdateUserProfileResult,
            user_service::use_cases::commands::update_user_profile::UpdateUserProfileError,
        > {
            unreachable!("unused update user profile")
        }
    }

    #[async_trait::async_trait]
    impl user_service::use_cases::commands::change_user_role::ChangeUserRoleUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: user_service::use_cases::commands::change_user_role::ChangeUserRoleCommand,
        ) -> Result<
            user_service::use_cases::commands::change_user_role::ChangeUserRoleResult,
            user_service::use_cases::commands::change_user_role::ChangeUserRoleError,
        > {
            unreachable!("unused change user role")
        }
    }

    #[async_trait::async_trait]
    impl user_service::use_cases::commands::change_user_tier::ChangeUserTierUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: user_service::use_cases::commands::change_user_tier::ChangeUserTierCommand,
        ) -> Result<
            user_service::use_cases::commands::change_user_tier::ChangeUserTierResult,
            user_service::use_cases::commands::change_user_tier::ChangeUserTierError,
        > {
            unreachable!("unused change user tier")
        }
    }
    #[async_trait::async_trait]
    impl user_service::use_cases::commands::delete_user::DeleteUserUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: user_service::use_cases::commands::delete_user::DeleteUserCommand,
        ) -> Result<
            user_service::use_cases::commands::delete_user::DeleteUserResult,
            user_service::use_cases::commands::delete_user::DeleteUserError,
        > {
            unreachable!("unused delete user")
        }
    }
    #[async_trait::async_trait]
    impl user_service::use_cases::queries::check_user_admin::CheckUserAdminUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: user_service::use_cases::queries::check_user_admin::CheckUserAdminRequest,
        ) -> Result<
            user_service::use_cases::queries::check_user_admin::CheckUserAdminResult,
            user_service::use_cases::queries::check_user_admin::CheckUserAdminError,
        > {
            unreachable!("unused check admin")
        }
    }
    #[async_trait::async_trait]
    impl user_service::use_cases::commands::create_access_token::CreateAccessTokenUseCase
        for UnusedUseCase
    {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: user_service::use_cases::commands::create_access_token::CreateAccessTokenCommand,
        ) -> Result<
            user_service::use_cases::commands::create_access_token::CreateAccessTokenResult,
            user_service::use_cases::commands::create_access_token::CreateAccessTokenError,
        > {
            unreachable!("unused create token")
        }
    }
    #[async_trait::async_trait]
    impl user_service::use_cases::queries::list_access_tokens::ListAccessTokensUseCase
        for UnusedUseCase
    {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: user_service::use_cases::queries::list_access_tokens::ListAccessTokensRequest,
        ) -> Result<
            user_service::use_cases::queries::list_access_tokens::ListAccessTokensResult,
            user_service::use_cases::queries::list_access_tokens::ListAccessTokensError,
        > {
            unreachable!("unused list tokens")
        }
    }
    #[async_trait::async_trait]
    impl user_service::use_cases::queries::get_access_token::GetAccessTokenUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: user_service::use_cases::queries::get_access_token::GetAccessTokenRequest,
        ) -> Result<
            user_service::use_cases::queries::get_access_token::AccessTokenView,
            user_service::use_cases::queries::get_access_token::GetAccessTokenError,
        > {
            unreachable!("unused get token")
        }
    }
    #[async_trait::async_trait]
    impl user_service::use_cases::commands::update_access_token::UpdateAccessTokenUseCase
        for UnusedUseCase
    {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: user_service::use_cases::commands::update_access_token::UpdateAccessTokenCommand,
        ) -> Result<
            user_service::use_cases::commands::update_access_token::UpdateAccessTokenResult,
            user_service::use_cases::commands::update_access_token::UpdateAccessTokenError,
        > {
            unreachable!("unused update token")
        }
    }
    #[async_trait::async_trait]
    impl user_service::use_cases::commands::delete_access_token::DeleteAccessTokenUseCase
        for UnusedUseCase
    {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: user_service::use_cases::commands::delete_access_token::DeleteAccessTokenCommand,
        ) -> Result<
            user_service::use_cases::commands::delete_access_token::DeleteAccessTokenResult,
            user_service::use_cases::commands::delete_access_token::DeleteAccessTokenError,
        > {
            unreachable!("unused delete token")
        }
    }

    #[async_trait::async_trait]
    impl watchlist_service::use_cases::ListWatchlistUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: watchlist_service::use_cases::ListWatchlistRequest,
        ) -> Result<
            watchlist_service::use_cases::ListWatchlistResult,
            watchlist_service::use_cases::ListWatchlistError,
        > {
            unreachable!("unused list watchlist")
        }
    }
    #[async_trait::async_trait]
    impl watchlist_service::use_cases::WatchProductUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: watchlist_service::use_cases::WatchProductCommand,
        ) -> Result<
            watchlist_service::use_cases::WatchProductResult,
            watchlist_service::use_cases::WatchProductError,
        > {
            unreachable!("unused watch product")
        }
    }
    #[async_trait::async_trait]
    impl watchlist_service::use_cases::UpdateWatchlistProductUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: watchlist_service::use_cases::UpdateWatchlistProductCommand,
        ) -> Result<
            watchlist_service::use_cases::UpdateWatchlistProductResult,
            watchlist_service::use_cases::UpdateWatchlistProductError,
        > {
            unreachable!("unused update watchlist")
        }
    }
    #[async_trait::async_trait]
    impl watchlist_service::use_cases::UnwatchProductUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: watchlist_service::use_cases::UnwatchProductCommand,
        ) -> Result<
            watchlist_service::use_cases::UnwatchProductResult,
            watchlist_service::use_cases::UnwatchProductError,
        > {
            unreachable!("unused delete watchlist")
        }
    }

    #[async_trait::async_trait]
    impl shop_partner_service::use_cases::CreatePartnerShopApplicationUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: shop_partner_service::use_cases::CreatePartnerShopApplicationCommand,
        ) -> Result<
            shop_partner_service::use_cases::CreatePartnerShopApplicationResult,
            shop_partner_service::use_cases::CreatePartnerShopApplicationError,
        > {
            unreachable!("unused create application")
        }
    }
    #[async_trait::async_trait]
    impl shop_partner_service::use_cases::ListPartnerShopApplicationsUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: shop_partner_service::use_cases::ListPartnerShopApplicationsRequest,
        ) -> Result<
            shop_partner_service::use_cases::ListPartnerShopApplicationsResult,
            shop_partner_service::use_cases::ListPartnerShopApplicationsError,
        > {
            unreachable!("unused list applications")
        }
    }
    #[async_trait::async_trait]
    impl shop_partner_service::use_cases::GetPartnerShopApplicationUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: shop_partner_service::use_cases::GetPartnerShopApplicationRequest,
        ) -> Result<
            shop_partner_service::use_cases::GetPartnerShopApplicationResult,
            shop_partner_service::use_cases::GetPartnerShopApplicationError,
        > {
            unreachable!("unused get application")
        }
    }
    #[async_trait::async_trait]
    impl shop_partner_service::use_cases::WithdrawPartnerShopApplicationUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: shop_partner_service::use_cases::WithdrawPartnerShopApplicationCommand,
        ) -> Result<(), shop_partner_service::use_cases::WithdrawPartnerShopApplicationError>
        {
            unreachable!("unused delete application")
        }
    }
    #[async_trait::async_trait]
    impl shop_partner_service::use_cases::AdminListPartnerShopApplicationsUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: shop_partner_service::use_cases::AdminListPartnerShopApplicationsRequest,
        ) -> Result<
            shop_partner_service::use_cases::AdminListPartnerShopApplicationsResult,
            shop_partner_service::use_cases::AdminListPartnerShopApplicationsError,
        > {
            unreachable!("unused admin list applications")
        }
    }
    #[async_trait::async_trait]
    impl shop_partner_service::use_cases::AdminGetPartnerShopApplicationUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: shop_partner_service::use_cases::AdminGetPartnerShopApplicationRequest,
        ) -> Result<
            shop_partner_service::use_cases::AdminGetPartnerShopApplicationResult,
            shop_partner_service::use_cases::AdminGetPartnerShopApplicationError,
        > {
            unreachable!("unused admin get application")
        }
    }
    #[async_trait::async_trait]
    impl shop_partner_service::use_cases::AdminUpdatePartnerShopApplicationUseCase for UnusedUseCase {
        async fn mark_in_review(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: shop_partner_service::use_cases::AdminMarkPartnerShopApplicationInReviewCommand,
        ) -> Result<
            shop_partner_service::use_cases::AdminUpdatePartnerShopApplicationResult,
            shop_partner_service::use_cases::AdminUpdatePartnerShopApplicationError,
        > {
            unreachable!("unused admin update application")
        }
    }
    #[async_trait::async_trait]
    impl shop_partner_service::use_cases::AdminDecidePartnerShopApplicationUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: shop_partner_service::use_cases::AdminDecidePartnerShopApplicationCommand,
        ) -> Result<
            shop_partner_service::use_cases::AdminDecidePartnerShopApplicationResult,
            shop_partner_service::use_cases::AdminDecidePartnerShopApplicationError,
        > {
            unreachable!("unused admin decide application")
        }
    }

    struct StaticAuthenticator;

    #[async_trait::async_trait]
    impl TokenAuthenticator for StaticAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::AuraAccessToken,
                capabilities: BTreeSet::from([CredentialCapability::ShopsRead]),
            })
        }
    }

    use tower::ServiceExt;
}
