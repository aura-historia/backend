pub(crate) mod admin_overview;
pub mod admin_product_listings;
pub mod auctions;
pub mod auth;
pub mod billing;
pub mod error;
mod geographic_search;
pub mod lambda;
pub mod listing_sources;
pub mod newsletter;
pub mod notifications;
pub mod oauth;
pub(crate) mod pagination_data;
pub mod parties;
pub mod partner_product_listings;
pub(crate) mod partnership_applications;
pub(crate) mod partnerships;
pub(crate) mod patch_value;
mod product_listing_management;
pub mod product_listings;
pub(crate) mod public_overview;
pub mod search_filters;
pub mod shipping;
mod shipping_search;
pub mod state;
pub mod transport;
pub mod users;
pub(crate) mod values;
pub mod watchlist;
pub mod webhooks;
pub(crate) mod wire;

use crate::auth::{
    ApiAuthService, AuraAccessTokenAuthenticator, AuthError, CognitoJwtAuthenticator,
    CognitoJwtConfig, JwksProvider, ReqwestJwksProvider, TokenAuthenticator,
    UserAuthenticationAuthenticator,
};
use crate::state::{
    AdminOverviewState, AppState, AsyncPartnerProductListingsState, AuctionsState, BillingState,
    ListingSourcesState, LoopsWebhooksState, NewsletterState, NotificationsState, OAuthState,
    PartiesState, PartnerProductListingsState, PartnershipApplicationsState, PartnershipsState,
    ProductListingsState, PublicAuctionsState, PublicListingSourceReadBudget, PublicOverviewState,
    ReadinessCheck, SearchFiltersState, UsersState, WatchlistState, WebhooksState,
};
use crate::transport::with_transport_middleware;
use admin_overview_postgres::SqlxAdminOverviewReaderFactory;
use admin_overview_service::GetAdminOverviewHandler;
use auction_postgres::{
    SqlxAdminAuctionSearchReader, SqlxAuctionDetailsReader, SqlxAuctionEventAppenderFactory,
    SqlxAuctionRepositoryFactory,
};
use auction_service::use_cases::{
    commands::{create_auction::CreateAuctionHandler, update_auction::UpdateAuctionHandler},
    queries::{get_auction::GetAuctionHandler, search_admin_auctions::SearchAdminAuctionsHandler},
};
pub use aura_historia_runtime::product_listing_config::{
    PRODUCT_LISTING_SEARCH_FX_CACHE_ENABLED_ENV, PRODUCT_LISTING_SEARCH_FX_CACHE_MAX_ENTRIES_ENV,
    PRODUCT_LISTING_SEARCH_FX_LATEST_TTL_SECONDS_ENV,
    PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV,
    PRODUCT_LISTING_SEARCH_SOURCE_CACHE_ENABLED_ENV,
    PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_BYTES_ENV,
    PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_ENTRIES_ENV,
    PRODUCT_LISTING_SEARCH_SOURCE_CACHE_TTL_SECONDS_ENV, VERTEX_AI_LOCATION_ENV,
    VERTEX_AI_PROJECT_ID_ENV,
};
use aura_historia_runtime::{ProductListingConfig, ProductListingDependencies, Runtime};
use axum::Router;
use axum::routing::{delete, get, patch, post};
use billing_service::use_cases::{
    BillingPriceIds, CreateBillingCheckoutSessionHandler, CreateBillingManagementSessionHandler,
    CreateBillingPortalSessionHandler,
};
use billing_stripe::{StripeBillingClient, StripeBillingConfig};
use embedding::VertexAiEmbeddingConfig;
use notification_postgres::{
    SqlxNotificationDeliveryIntentRepositoryFactory, SqlxNotificationRepositoryFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use oauth_postgres::{
    SqlxAuthorizationCodeRepositoryFactory, SqlxOAuthClientAuthenticationReader,
    SqlxOAuthClientDetailsReader, SqlxOAuthClientListReader, SqlxOAuthClientRepositoryFactory,
    SqlxThirdPartyExchangeCodeRepositoryFactory,
};
use oauth_service::use_cases::{
    AuthorizeHandler, CreateOAuthClientHandler, DeleteOAuthClientHandler, GetOAuthClientHandler,
    GetOAuthConsentClientHandler, IntrospectTokenHandler, ListOAuthClientsHandler,
    RevokeTokenHandler, TokenByAuthorizationCodeHandler, TokenByThirdPartyCodeHandler,
    UpdateOAuthClientHandler,
};
use opensearch::{OpenSearch, SearchParts};
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::SqlxAdminProductListingReader;
use product_listing_service::use_cases::{
    commands::admin_product_listings::AdminProductListingMutationHandler,
    queries::admin_product_listings::{
        GetAdminProductListingHandler, SearchAdminProductListingsHandler,
    },
};
use woocommerce_service::WoocommerceWebhookIntake;

use listing_source_postgres::{
    SqlxListingSourceReaders, SqlxListingSourceRepositoryFactory,
    SqlxListingSourceSearchReaderFactory,
};
use listing_source_service::use_cases::commands::create_listing_source::CreateListingSourceHandler;
use listing_source_service::use_cases::commands::delete_listing_source::DeleteListingSourceHandler;
use listing_source_service::use_cases::commands::update_listing_source::UpdateListingSourceHandler;
use listing_source_service::use_cases::queries::get_listing_source::GetListingSourceHandler;
use listing_source_service::use_cases::queries::search_listing_sources::SearchListingSourcesHandler;
use partnership_postgres::{
    SqlxListingSourceGrantRepositoryFactory, SqlxPartnershipApplicationReaderFactory,
    SqlxPartnershipApplicationRepositoryFactory, SqlxPartnershipDetailsReaderFactory,
    SqlxPartnershipRepositoryFactory, SqlxPartnershipSearchReaderFactory,
};
use partnership_service::use_cases::{
    commands::{
        approve_partnership_application::ApprovePartnershipApplicationHandler,
        dissolve_partnership::DissolvePartnershipHandler,
        grant_partnership_listing_source::GrantPartnershipListingSourceHandler,
        grant_partnership_membership::GrantPartnershipMembershipHandler,
        mark_partnership_application_in_review::MarkPartnershipApplicationInReviewHandler,
        reject_partnership_application::RejectPartnershipApplicationHandler,
        revoke_partnership_listing_source::RevokePartnershipListingSourceHandler,
        revoke_partnership_membership::RevokePartnershipMembershipHandler,
        submit_partnership_application::SubmitPartnershipApplicationHandler,
        withdraw_partnership_application::WithdrawPartnershipApplicationHandler,
    },
    queries::{
        get_admin_partnership::GetAdminPartnershipHandler,
        get_own_partnership_application::GetOwnPartnershipApplicationHandler,
        get_partnership_application::GetPartnershipApplicationHandler,
        list_admin_partnership_applications::ListAdminPartnershipApplicationsHandler,
        list_admin_partnerships::ListAdminPartnershipsHandler,
        list_own_partnership_applications::ListOwnPartnershipApplicationsHandler,
    },
};
use party_postgres::{SqlxPartyRepositoryFactory, SqlxPartySearchReaderFactory};
use party_service::use_cases::commands::create_party::CreatePartyHandler;
use party_service::use_cases::commands::delete_party::DeletePartyHandler;
use party_service::use_cases::commands::update_party::UpdatePartyHandler;
use party_service::use_cases::queries::get_party::GetPartyHandler;
use party_service::use_cases::queries::list_party_location_managers::ListPartyLocationManagersHandler;
use party_service::use_cases::queries::search_parties::SearchPartiesHandler;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::use_cases::{
    AuthorizeProductListingRawCaptureHandler, ProductListingSearchReadExecutionPolicy,
};

use sqlx::PgPool;
use std::future::Future;
use std::net::{AddrParseError, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tracing::info;
use user_cognito::CognitoUserSessionRevoker;
use user_email_aws::{NewsletterConfirmationEmailConfig, SesNewsletterConfirmationEmailSender};
use user_loops::{
    LoopsMarketingEmailConsentWriter, LoopsNewsletterConfig, LoopsNewsletterWebhookVerifier,
};
use user_postgres::{
    SqlxAccessTokenAuthenticationReader, SqlxAccessTokenDetailsReader, SqlxAccessTokenListReader,
    SqlxAccessTokenRepositoryFactory, SqlxAdminAccessTokenListReaderFactory,
    SqlxCognitoUserIdentityReader, SqlxLoopsWebhookReceiptRepository,
    SqlxMarketingConsentIntentRepository, SqlxNewsletterConfirmationChallengesRepository,
    SqlxNewsletterProfileReader, SqlxUserAccountReaderFactory, SqlxUserAdminReaderFactory,
    SqlxUserAuthenticationReader, SqlxUserCognitoIdentityRegistryFactory,
    SqlxUserRepositoryFactory, SqlxUserSearchReaderFactory, SqlxUserTierEntitlementsFactory,
};
use user_service::ports::NewsletterWebhookMailingListId;
use user_service::use_cases::ApplyLoopsPreferenceEventHandler;
use user_service::use_cases::commands::associate_user_stripe_customer_id::AssociateUserStripeCustomerIdHandler;
use user_service::use_cases::commands::change_user_role::ChangeUserRoleHandler;
use user_service::use_cases::commands::change_user_tier::ChangeUserTierHandler;
use user_service::use_cases::commands::create_access_token::CreateAccessTokenHandler;
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenHandler;
use user_service::use_cases::commands::delete_access_tokens::DeleteAccessTokensHandler;
use user_service::use_cases::commands::delete_user::DeleteUserHandler;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenHandler;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileHandler;
use user_service::use_cases::queries::admin_get_user::AdminGetUserHandler;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminHandler;
use user_service::use_cases::queries::get_access_token::GetAccessTokenHandler;
use user_service::use_cases::queries::get_own_user::GetOwnUserHandler;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensHandler;
use user_service::use_cases::queries::list_admin_access_tokens::ListAdminAccessTokensHandler;
use user_service::use_cases::queries::search_users::SearchUsersHandler;
use user_service::use_cases::{
    AuthenticateAccessTokenHandler, AuthenticateUserHandler, ConfirmNewsletterSubscriptionHandler,
    RequestNewsletterSubscriptionHandler, ResolveCognitoUserHandler, RevokeUserSessionsHandler,
    SuspendUserHandler, UnsuspendUserHandler,
};

pub const API_BIND_ADDR_ENV: &str = "AURA_HISTORIA_API_BIND_ADDR";
pub const COGNITO_ISSUER_ENV: &str = "AURA_HISTORIA_COGNITO_ISSUER";
pub const COGNITO_JWKS_URL_ENV: &str = "AURA_HISTORIA_COGNITO_JWKS_URL";
pub const COGNITO_APP_CLIENT_IDS_ENV: &str = "AURA_HISTORIA_COGNITO_APP_CLIENT_IDS";
pub const COGNITO_USER_POOL_ID_ENV: &str = "AURA_HISTORIA_COGNITO_USER_POOL_ID";
pub const STRIPE_API_KEY_ENV: &str = "STRIPE_API_KEY";
pub const STRIPE_CHECKOUT_SUCCESS_URL_ENV: &str = "STRIPE_CHECKOUT_SUCCESS_URL";
pub const STRIPE_CHECKOUT_CANCEL_URL_ENV: &str = "STRIPE_CHECKOUT_CANCEL_URL";
pub const STRIPE_PORTAL_RETURN_URL_ENV: &str = "STRIPE_PORTAL_RETURN_URL";
pub const STRIPE_PRO_MONTHLY_PRICE_ID_ENV: &str = "STRIPE_PRO_MONTHLY_PRICE_ID";
pub const STRIPE_PRO_YEARLY_PRICE_ID_ENV: &str = "STRIPE_PRO_YEARLY_PRICE_ID";
pub const STRIPE_ULTIMATE_MONTHLY_PRICE_ID_ENV: &str = "STRIPE_ULTIMATE_MONTHLY_PRICE_ID";
pub const STRIPE_ULTIMATE_YEARLY_PRICE_ID_ENV: &str = "STRIPE_ULTIMATE_YEARLY_PRICE_ID";
pub const LOOPS_API_KEY_ENV: &str = "LOOPS_API_KEY";
pub const LOOPS_NEWSLETTER_LIST_ID_ENV: &str = "LOOPS_NEWSLETTER_LIST_ID";
pub const LOOPS_API_BASE_URL_ENV: &str = "LOOPS_API_BASE_URL";
pub const LOOPS_WEBHOOK_SIGNING_SECRET_ENV: &str = "LOOPS_WEBHOOK_SIGNING_SECRET";
pub const NEWSLETTER_CONFIRMATION_FRONTEND_ORIGIN_ENV: &str =
    "NEWSLETTER_CONFIRMATION_FRONTEND_ORIGIN";
pub const NEWSLETTER_CONFIRMATION_EMAIL_FROM_ENV: &str = "NEWSLETTER_CONFIRMATION_EMAIL_FROM";
pub const NEWSLETTER_CONFIRMATION_EMAIL_REPLY_TO_ENV: &str =
    "NEWSLETTER_CONFIRMATION_EMAIL_REPLY_TO";
pub const NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET_ENV: &str =
    "NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET";
pub const S3_BUCKET_NAME_TEMPLATES_ENV: &str = "S3_BUCKET_NAME_TEMPLATES";
pub const OAUTH_ISSUER_URL_ENV: &str = "OAUTH_ISSUER_URL";
pub const STAGE_ENV: &str = "STAGE";
pub const COMMIT_SHA_ENV: &str = "COMMIT_SHA";
pub const MAIL_TEMPLATE_PREFIX_ENV: &str = "MAIL_TEMPLATE_PREFIX";
pub const PRODUCT_LISTING_INGESTION_QUEUE_URL_ENV: &str = "PRODUCT_LISTING_INGESTION_QUEUE_URL";
pub const PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT_ENV: &str =
    "PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT";
pub const PUBLIC_LISTING_SOURCE_READ_REQUEST_TIMEOUT_MS_ENV: &str =
    "PUBLIC_LISTING_SOURCE_READ_REQUEST_TIMEOUT_MS";
const DEFAULT_PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT: u64 = 4;
const DEFAULT_PUBLIC_LISTING_SOURCE_READ_REQUEST_TIMEOUT_MS: u64 = 500;
const DEFAULT_API_BIND_ADDR: &str = "0.0.0.0:8080";
const JWKS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const JWKS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_LOOPS_API_BASE_URL: &str = "https://app.loops.so/api";

#[derive(Clone, PartialEq, Eq)]
pub struct ApiConfig {
    bind_addr: SocketAddr,
    cognito_jwt: CognitoJwtConfig,
    oauth_resource_policy: oauth_core::resource::AuraResourcePolicy,
    cognito_user_pool_id: String,
    product_listings: ProductListingConfig,
    stripe_billing: StripeBillingConfig,
    billing_prices: BillingPriceIds,
    loops: LoopsNewsletterConfig,
    newsletter_confirmation_email: NewsletterConfirmationEmailConfig,
    loops_webhook_signing_secret: String,
    public_listing_source_read_max_in_flight: usize,
    public_listing_source_read_request_timeout: Duration,
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

        let oauth_resource_policy = oauth_core::resource::AuraResourcePolicy::from_issuer(
            &required_config(&mut get, OAUTH_ISSUER_URL_ENV)?,
        )
        .map_err(ApiConfigError::InvalidOAuthIssuer)?;
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
        let cognito_user_pool_id = required_config(&mut get, COGNITO_USER_POOL_ID_ENV)?;
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

        let product_listings = ProductListingConfig::from_getter(&mut get)?;
        let public_listing_source_read_max_in_flight = optional_bounded_usize_config(
            &mut get,
            PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT_ENV,
            DEFAULT_PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT,
            1,
            64,
        )?;
        let public_listing_source_read_request_timeout =
            Duration::from_millis(optional_bounded_u64_config(
                &mut get,
                PUBLIC_LISTING_SOURCE_READ_REQUEST_TIMEOUT_MS_ENV,
                DEFAULT_PUBLIC_LISTING_SOURCE_READ_REQUEST_TIMEOUT_MS,
                1,
                5_000,
            )?);
        let loops = LoopsNewsletterConfig::new(
            get(LOOPS_API_KEY_ENV).unwrap_or_default(),
            get(LOOPS_NEWSLETTER_LIST_ID_ENV).unwrap_or_default(),
            get(LOOPS_API_BASE_URL_ENV).unwrap_or_else(|| DEFAULT_LOOPS_API_BASE_URL.to_owned()),
        )
        .map_err(ApiConfigError::LoopsConfig)?;
        let newsletter_confirmation_email = NewsletterConfirmationEmailConfig::new(
            required_config(&mut get, S3_BUCKET_NAME_TEMPLATES_ENV)?,
            required_config(&mut get, NEWSLETTER_CONFIRMATION_EMAIL_FROM_ENV)?,
            required_config(&mut get, NEWSLETTER_CONFIRMATION_EMAIL_REPLY_TO_ENV)?,
            required_config(
                &mut get,
                NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET_ENV,
            )?,
            required_config(&mut get, STAGE_ENV)?,
            required_config(&mut get, MAIL_TEMPLATE_PREFIX_ENV)?,
            &required_config(&mut get, NEWSLETTER_CONFIRMATION_FRONTEND_ORIGIN_ENV)?,
        )
        .map_err(ApiConfigError::NewsletterConfirmationEmailConfig)?;
        let loops_webhook_signing_secret =
            required_config(&mut get, LOOPS_WEBHOOK_SIGNING_SECRET_ENV)?;
        user_loops::LoopsNewsletterWebhookVerifier::validate_signing_secret(
            &loops_webhook_signing_secret,
        )
        .map_err(|_| ApiConfigError::InvalidLoopsWebhookSigningSecret)?;

        Ok(Self {
            bind_addr,
            cognito_jwt: CognitoJwtConfig::new(issuer, jwks_url, app_client_ids),
            oauth_resource_policy,
            cognito_user_pool_id,
            product_listings,
            stripe_billing,
            billing_prices,
            loops,
            newsletter_confirmation_email,
            loops_webhook_signing_secret,
            public_listing_source_read_max_in_flight,
            public_listing_source_read_request_timeout,
        })
    }

    pub const fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub fn oauth_resource_policy(&self) -> oauth_core::resource::AuraResourcePolicy {
        self.oauth_resource_policy
    }

    pub fn cognito_jwt(&self) -> &CognitoJwtConfig {
        &self.cognito_jwt
    }

    pub fn cognito_user_pool_id(&self) -> &str {
        &self.cognito_user_pool_id
    }

    pub fn vertex_ai_embedding(&self) -> &VertexAiEmbeddingConfig {
        self.product_listings.vertex_ai_embedding()
    }

    fn stripe_billing(&self) -> &StripeBillingConfig {
        &self.stripe_billing
    }

    fn billing_prices(&self) -> &BillingPriceIds {
        &self.billing_prices
    }

    fn loops(&self) -> &LoopsNewsletterConfig {
        &self.loops
    }

    fn newsletter_confirmation_email(&self) -> &NewsletterConfirmationEmailConfig {
        &self.newsletter_confirmation_email
    }

    fn loops_webhook_signing_secret(&self) -> &str {
        &self.loops_webhook_signing_secret
    }

    /// Shared, non-queuing public ListingSource read budget for this API process.
    fn public_listing_source_read_budget(&self) -> PublicListingSourceReadBudget {
        PublicListingSourceReadBudget::new(
            self.public_listing_source_read_max_in_flight,
            self.public_listing_source_read_request_timeout,
        )
    }
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

fn optional_bounded_u64_config<F>(
    get: &mut F,
    name: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ApiConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let Some(value) = get(name) else {
        return Ok(default);
    };
    let parsed = value
        .parse::<u64>()
        .map_err(|_| ApiConfigError::InvalidIntegerConfig {
            name,
            value: value.clone(),
        })?;
    if !(min..=max).contains(&parsed) {
        return Err(ApiConfigError::OutOfRangeIntegerConfig {
            name,
            value,
            min,
            max,
        });
    }
    Ok(parsed)
}

fn optional_bounded_usize_config<F>(
    get: &mut F,
    name: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<usize, ApiConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let value = optional_bounded_u64_config(get, name, default, min, max)?;
    usize::try_from(value).map_err(|_| ApiConfigError::OutOfRangeIntegerConfig {
        name,
        value: value.to_string(),
        min,
        max,
    })
}

#[derive(thiserror::Error, Debug)]
pub enum ApiConfigError {
    #[error("OAUTH_ISSUER_URL is not a configured Aura issuer")]
    InvalidOAuthIssuer(#[source] oauth_core::resource::InvalidResource),
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
    #[error("invalid integer configuration {name}: {value}")]
    InvalidIntegerConfig { name: &'static str, value: String },
    #[error("integer configuration {name} must be between {min} and {max}: {value}")]
    OutOfRangeIntegerConfig {
        name: &'static str,
        value: String,
        min: u64,
        max: u64,
    },
    #[error(transparent)]
    ProductListings(
        #[from] aura_historia_runtime::product_listing_config::ProductListingConfigError,
    ),
    #[error("invalid Loops newsletter configuration")]
    LoopsConfig(#[source] user_loops::LoopsNewsletterConfigError),
    #[error("invalid newsletter confirmation email configuration")]
    NewsletterConfirmationEmailConfig(
        #[source] user_email_aws::NewsletterConfirmationEmailConfigError,
    ),
    #[error("invalid Loops webhook signing-secret configuration")]
    InvalidLoopsWebhookSigningSecret,
}

pub fn app(state: AppState) -> Router {
    app_with_request_timeout(state, crate::transport::NATIVE_REQUEST_TIMEOUT)
}

fn app_with_request_timeout(state: AppState, request_timeout: Duration) -> Router {
    let health_routes = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/ready", get(ready))
        .with_state(Arc::clone(&state.readiness));
    let mut routes = health_routes;

    if let Some(products) = state.product_listings {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/product-listings",
                    get(product_listings::search_products::get_products),
                )
                .route(
                    "/v1/product-listings/{product_listing_id}",
                    get(product_listings::get_product_by_id::get_product_by_id),
                )
                .route(
                    "/v1/product-listings/by-slug/{product_listing_title_slug_id}",
                    get(product_listings::get_product_by_title_slug::get_product_by_title_slug),
                )
                .route(
                    "/v1/product-listings/{product_listing_id}/history",
                    get(product_listings::get_product_listing_history::get_product_listing_history_by_id),
                )
                .route(
                    "/v1/product-listings/{product_listing_id}/similar",
                    get(product_listings::get_similar_products::get_similar_products_by_id),
                )
                .with_state(products),
        );
    }

    if let Some(webhooks) = state.webhooks {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/webhooks/woocommerce/{listing_source_id}",
                    post(webhooks::post_woocommerce::post_woocommerce),
                )
                .with_state(webhooks),
        );
    }

    if let Some(webhooks) = state.loops_webhooks {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/webhooks/loops",
                    post(webhooks::post_loops::post_loops).layer(
                        tower_http::limit::RequestBodyLimitLayer::new(
                            crate::transport::MAX_LOOPS_WEBHOOK_BODY_BYTES,
                        ),
                    ),
                )
                .with_state(webhooks),
        );
    }

    if let Some(partner_product_listings) = state.partner_product_listings {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/listing-sources/{listingSourceId}/product-listings",
                    post(partner_product_listings::create_products::create_products)
                        .patch(partner_product_listings::update_products::update_products)
                        .put(partner_product_listings::upsert_products::upsert_products)
                        .delete(partner_product_listings::delete_products::delete_products),
                )
                .with_state(partner_product_listings),
        );
    }

    if let Some(management) = state.partner_product_listing_management {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/listing-sources/{listingSourceId}/product-listings",
                    get(partner_product_listings::management::list_partner_product_listings),
                )
                .route(
                    "/v1/listing-sources/{listingSourceId}/product-listings/{productListingId}",
                    get(partner_product_listings::management::get_partner_product_listing)
                        .patch(partner_product_listings::management::update_partner_product_listing)
                        .delete(partner_product_listings::management::withdraw_partner_product_listing),
                )
                .route(
                    "/v1/listing-sources/{listingSourceId}/product-listings/{productListingId}/restore",
                    post(partner_product_listings::management::restore_partner_product_listing),
                )
                .layer(axum::middleware::map_response(product_listing_management::no_store))
                .with_state(management),
        );
    }

    if let Some(async_partner_product_listings) = state.async_partner_product_listings {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/listing-sources/{listingSourceId}/product-listings/async",
                    post(partner_product_listings::async_create_products::create_products)
                        .patch(partner_product_listings::async_update_products::update_products)
                        .put(partner_product_listings::async_upsert_products::upsert_products)
                        .delete(partner_product_listings::async_delete_products::delete_products),
                )
                .with_state(async_partner_product_listings),
        );
    }

    if let Some(public_auctions) = state.public_auctions {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/auctions",
                    get(auctions::public_auctions::list_public_auctions),
                )
                .route(
                    "/v1/auctions/{auction_id}",
                    get(auctions::public_auctions::get_public_auction),
                )
                .route(
                    "/v1/auctions/{auction_id}/product-listings",
                    get(auctions::public_auctions::get_auction_catalogue),
                )
                .with_state(public_auctions),
        );
    }

    if let Some(admin_product_listings) = state.admin_product_listings {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/admin/product-listings",
                    get(admin_product_listings::search_admin_product_listings),
                )
                .route(
                    "/v1/admin/product-listings/{product_listing_id}",
                    get(admin_product_listings::get_admin_product_listing)
                        .patch(admin_product_listings::update_admin_product_listing)
                        .delete(admin_product_listings::withdraw_admin_product_listing),
                )
                .route(
                    "/v1/admin/product-listings/{product_listing_id}/restore",
                    post(admin_product_listings::restore_admin_product_listing),
                )
                .layer(axum::middleware::map_response(
                    admin_product_listings::no_store,
                ))
                .with_state(admin_product_listings),
        );
    }

    if let Some(auctions) = state.auctions {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/admin/auctions",
                    get(auctions::search_admin_auctions::search_admin_auctions)
                        .post(auctions::create_auction::create_auction),
                )
                .route(
                    "/v1/admin/auctions/{auction_id}",
                    get(auctions::get_auction::get_auction)
                        .patch(auctions::update_auction::update_auction),
                )
                .with_state(auctions),
        );
    }

    if let Some(admin_overview) = state.admin_overview {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/admin/overview",
                    get(admin_overview::get_admin_overview),
                )
                .with_state(admin_overview),
        );
    }

    if let Some(public_overview) = state.public_overview {
        routes = routes.merge(
            Router::new()
                .route("/v1/overview", get(public_overview::get_public_overview))
                .with_state(public_overview),
        );
    }

    if let Some(listing_sources) = state.listing_sources {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/admin/listing-sources/{listing_source_id}",
                    get(listing_sources::get_listing_source::get_listing_source)
                        .patch(listing_sources::update_listing_source::update_listing_source)
                        .delete(listing_sources::delete_listing_source::delete_listing_source),
                )
                .route(
                    "/v1/listing-sources/{listing_source_id}/ingestion-configurations/woocommerce",
                    axum::routing::put(listing_sources::put_ingestion_configuration::put_woocommerce_configuration),
                )
                .route(
                    "/v1/listing-sources/{listing_source_id}/ingestion-configurations/shopify",
                    axum::routing::put(listing_sources::put_ingestion_configuration::put_shopify_configuration),
                )
                .route(
                    "/v1/listing-sources",
                    get(listing_sources::search_public_listing_sources::search_public_listing_sources),
                )
                .route(
                    "/v1/listing-sources/by-slug/{listing_source_slug_id}",
                    get(listing_sources::get_listing_source_by_slug::get_listing_source_by_slug),
                )
                .route(
                    "/v1/me/listing-sources",
                    get(listing_sources::list_my_listing_sources::list_my_listing_sources),
                )
                .route(
                    "/v1/admin/listing-sources",
                    get(listing_sources::search_listing_sources::search_listing_sources)
                        .post(listing_sources::create_listing_source::create_listing_source),
                )
                .with_state(listing_sources),
        );
    }

    if let Some(oauth) = state.oauth {
        routes = routes.merge(
            Router::new()
                .route(
                    "/.well-known/oauth-authorization-server",
                    get(oauth::discovery::discovery),
                )
                .route(
                    "/v1/admin/oauth-clients",
                    get(oauth::list_clients::list_clients)
                        .post(oauth::create_client::create_client),
                )
                .route(
                    "/v1/admin/oauth-clients/{client_id}",
                    get(oauth::get_client::get_client)
                        .patch(oauth::update_client::update_client)
                        .delete(oauth::delete_client::delete_client),
                )
                .route(
                    "/v1/oauth/clients/{client_id}",
                    get(oauth::get_consent_client::get_consent_client),
                )
                .route("/v1/oauth/authorize", get(oauth::authorize::authorize))
                .route("/v1/oauth/token", post(oauth::token::token))
                .route(
                    "/v1/oauth/tokens/by-third-party-code/{third_party_code}",
                    get(oauth::token_by_third_party_code::token_by_third_party_code),
                )
                .route("/v1/oauth/revoke", post(oauth::revoke::revoke))
                .route("/v1/oauth/introspect", post(oauth::introspect::introspect))
                .with_state(oauth),
        );
    }

    if let Some(addresses) = state.user_addresses {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/me/addresses",
                    get(users::addresses::list).post(users::addresses::create),
                )
                .route(
                    "/v1/me/addresses/{address_id}",
                    get(users::addresses::get)
                        .put(users::addresses::update)
                        .delete(users::addresses::delete),
                )
                .route(
                    "/v1/me/browsing-destination",
                    get(users::addresses::get_destination).put(users::addresses::set_destination),
                )
                .layer(axum::middleware::map_response(
                    parties::locations::no_store_response,
                ))
                .with_state(addresses),
        );
    }
    if let Some(delivery) = state.product_listing_delivery {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/listing-sources/{listing_source_id}/product-listings/by-source-id/{source_listing_id}/delivery",
                    get(partner_product_listings::delivery::get)
                        .patch(partner_product_listings::delivery::change),
                )
                .route(
                    "/v1/listing-sources/{listing_source_id}/product-listings/by-source-id/{source_listing_id}/delivery/async",
                    axum::routing::post(partner_product_listings::delivery::submit),
                )
                .layer(axum::middleware::map_response(
                    parties::locations::no_store_response,
                ))
                .with_state(delivery),
        );
    }

    if let Some(shipping) = state.shipping {
        routes = routes.merge(
            Router::new()
                .route("/v1/listing-sources/{listing_source_id}/shipping-profiles", get(shipping::list).post(shipping::create))
                .route("/v1/listing-sources/{listing_source_id}/shipping-profiles/{shipping_profile_id}", get(shipping::get).put(shipping::replace).delete(shipping::retire))
                .route("/v1/listing-sources/{listing_source_id}/shipping-profiles/{shipping_profile_id}/restore", axum::routing::post(shipping::restore))
                .route("/v1/listing-sources/{listing_source_id}/shipping-default", get(shipping::get_default).put(shipping::assign_default).delete(shipping::clear_default))
                .layer(axum::middleware::map_response(parties::locations::no_store_response))
                .with_state(shipping),
        );
    }
    if let Some(defaults) = state.source_location_defaults {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/listing-sources/{listing_source_id}/location-default",
                    get(listing_sources::location_default::get)
                        .put(listing_sources::location_default::assign)
                        .patch(listing_sources::location_default::change)
                        .delete(listing_sources::location_default::clear),
                )
                .layer(axum::middleware::map_response(
                    parties::locations::no_store_response,
                ))
                .with_state(defaults),
        );
    }
    if let Some(defaults) = state.admin_source_location_defaults {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/admin/listing-sources/{listing_source_id}/location-default",
                    get(listing_sources::location_default::get)
                        .put(listing_sources::location_default::assign)
                        .patch(listing_sources::location_default::change)
                        .delete(listing_sources::location_default::clear),
                )
                .layer(axum::middleware::map_response(
                    parties::locations::no_store_response,
                ))
                .with_state(defaults),
        );
    }
    if let Some(locations) = state.party_locations {
        if let Some(discovery) = locations.discovery.clone() {
            routes = routes.merge(
                Router::new()
                    .route(
                        "/v1/party-locations",
                        get(parties::discover_locations::discover),
                    )
                    .with_state(discovery),
            );
        }
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/parties/{party_id}/locations",
                    get(parties::locations::list).post(parties::locations::create),
                )
                .route(
                    "/v1/parties/{party_id}/locations/{location_id}",
                    get(parties::locations::get).patch(parties::locations::update),
                )
                .route(
                    "/v1/parties/{party_id}/locations/{location_id}/lifecycle",
                    axum::routing::put(parties::locations::lifecycle),
                )
                .route(
                    "/v1/admin/parties/{party_id}/location-managers",
                    get(parties::list_location_managers::list),
                )
                .route(
                    "/v1/admin/parties/{party_id}/location-managers/{user_id}",
                    axum::routing::put(parties::locations::grant),
                )
                .layer(axum::middleware::map_response(
                    parties::locations::no_store_response,
                ))
                .with_state(locations),
        );
    }
    if let Some(parties) = state.parties {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/admin/parties",
                    get(parties::search_parties::search_parties)
                        .post(parties::create_party::create_party),
                )
                .route(
                    "/v1/admin/parties/{party_id}",
                    get(parties::get_party::get_party)
                        .patch(parties::update_party::update_party)
                        .delete(parties::delete_party::delete_party),
                )
                .with_state(parties),
        );
    }

    if let Some(users) = state.users {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/me/account",
                    get(users::account::get_me).patch(users::account::patch_me),
                )
                .route("/v1/me", delete(users::account::delete_me))
                .route(
                    "/v1/me/access-tokens",
                    get(users::access_tokens::list_access_tokens)
                        .post(users::access_tokens::post_access_token)
                        .patch(users::access_tokens::patch_access_token),
                )
                .route(
                    "/v1/me/access-tokens/{access_token_id}",
                    get(users::access_tokens::get_access_token)
                        .delete(users::access_tokens::delete_access_token),
                )
                .route("/v1/admin/users", get(users::admin_users::search_users))
                .route(
                    "/v1/admin/users/{user_id}",
                    get(users::admin_users::get_user)
                        .patch(users::admin_users::patch_admin_user)
                        .delete(users::admin_users::delete_admin_user),
                )
                .route(
                    "/v1/admin/users/{user_id}/suspension",
                    axum::routing::put(users::suspend_user::suspend_user)
                        .delete(users::unsuspend_user::unsuspend_user),
                )
                .route(
                    "/v1/admin/users/{user_id}/sessions/revoke",
                    post(users::revoke_user_sessions::revoke_user_sessions),
                )
                .route(
                    "/v1/admin/users/{user_id}/access-tokens",
                    get(users::access_tokens::list_admin_access_tokens)
                        .delete(users::access_tokens::delete_admin_access_tokens),
                )
                .route(
                    "/v1/admin/users/{user_id}/access-tokens/{access_token_id}",
                    delete(users::access_tokens::delete_admin_access_token),
                )
                .with_state(users),
        );
    }

    if let Some(watchlist) = state.watchlist {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/me/watchlist",
                    get(watchlist::list::list_watchlist).post(watchlist::create::post_watchlist),
                )
                .route(
                    "/v1/me/watchlist/{product_listing_id}",
                    patch(watchlist::update::patch_watchlist)
                        .delete(watchlist::delete::delete_watchlist),
                )
                .with_state(watchlist),
        );
    }

    if let Some(search_filters) = state.search_filters {
        routes = routes.merge(search_filters::router(search_filters));
    }

    if let Some(notifications) = state.notifications {
        routes = routes.merge(notifications::router(notifications));
    }

    if let Some(billing) = state.billing {
        routes = routes.merge(
            Router::new()
                .route("/v1/me/billing/checkout", post(billing::checkout))
                .route("/v1/me/billing/portal", post(billing::portal))
                .route("/v1/me/billing/manage", post(billing::manage))
                .with_state(billing),
        );
    }

    if let Some(newsletter) = state.newsletter {
        routes = routes.merge(
            Router::new()
                .route(
                    "/v1/newsletter-subscriptions",
                    axum::routing::put(newsletter::put::put_newsletter_subscription),
                )
                .route(
                    "/v1/newsletter-subscriptions/confirm",
                    post(newsletter::confirm::confirm_newsletter_subscription),
                )
                .with_state(newsletter),
        );
    }

    if let Some(partnership_applications) = state.partnership_applications {
        routes = routes.merge(partnership_applications::router(partnership_applications));
    }

    if let Some(partnerships) = state.partnerships {
        routes = routes.merge(partnerships::router(partnerships));
    }

    with_transport_middleware(routes, request_timeout)
}

async fn health() -> &'static str {
    "ok\n"
}

const READINESS_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

async fn ready(
    axum::extract::State(readiness): axum::extract::State<Arc<dyn ReadinessCheck>>,
) -> axum::http::StatusCode {
    match tokio::time::timeout(READINESS_CHECK_TIMEOUT, readiness.check()).await {
        Ok(Ok(())) => axum::http::StatusCode::NO_CONTENT,
        _ => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    }
}

struct RuntimeReadiness {
    postgres: PgPool,
    opensearch: OpenSearch,
}

#[derive(serde::Deserialize)]
struct ReadinessSearchResponse {
    timed_out: bool,
    #[serde(rename = "_shards")]
    shards: ReadinessShardStats,
    hits: ReadinessHits,
    error: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct ReadinessShardStats {
    total: u64,
    successful: u64,
    failed: u64,
}

#[derive(serde::Deserialize)]
struct ReadinessHits {
    total: ReadinessHitCount,
    hits: Vec<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct ReadinessHitCount {
    #[serde(rename = "value")]
    _value: u64,
    relation: String,
}

use async_trait::async_trait;
use aws_config::BehaviorVersion;

#[async_trait]
impl ReadinessCheck for RuntimeReadiness {
    async fn check(&self) -> Result<(), ()> {
        let value: i32 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&self.postgres)
            .await
            .map_err(|_| ())?;
        if value != 1 {
            return Err(());
        }
        let response = self
            .opensearch
            .search(SearchParts::Index(&["product-listings"]))
            .body(serde_json::json!({ "size": 0, "query": { "match_all": {} } }))
            .send()
            .await
            .map_err(|_| ())?;
        if !response.status_code().is_success() {
            return Err(());
        }
        let search: ReadinessSearchResponse = response.json().await.map_err(|_| ())?;
        if search.timed_out
            || search.error.is_some()
            || search.shards.total == 0
            || search.shards.successful == 0
            || search.shards.failed != 0
            || !matches!(search.hits.total.relation.as_str(), "eq" | "gte")
            || !search.hits.hits.is_empty()
        {
            return Err(());
        }
        Ok(())
    }
}

pub async fn app_state_from_env() -> Result<AppState, ApiStateError> {
    let config = ApiConfig::from_env().map_err(ApiStateError::Config)?;
    app_state_from_config(&config).await
}

pub async fn lambda_app_from_env() -> Result<Router, ApiStateError> {
    let config = ApiConfig::from_env().map_err(ApiStateError::Config)?;
    let runtime = Runtime::from_env().await?;
    lambda_app_from_config_and_runtime(&config, &runtime).await
}

/// Composes a complete Lambda router for one PostgreSQL credential version.
pub async fn lambda_app_from_config_and_runtime(
    config: &ApiConfig,
    runtime: &Runtime,
) -> Result<Router, ApiStateError> {
    log_product_listing_search_cache_config(config);
    let state = app_state_from_config_and_runtime(config, runtime).await?;
    Ok(app_with_request_timeout(
        state,
        crate::transport::LAMBDA_REQUEST_TIMEOUT,
    ))
}

async fn app_state_from_config(config: &ApiConfig) -> Result<AppState, ApiStateError> {
    let runtime = Runtime::from_env().await?;
    app_state_from_config_and_runtime(config, &runtime).await
}

async fn app_state_from_config_and_runtime(
    config: &ApiConfig,
    runtime: &Runtime,
) -> Result<AppState, ApiStateError> {
    let pool = runtime.postgres_pool().clone();
    let public_listing_sources = runtime.public_listing_sources();
    let aws_client_config = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let ingestion_dependencies =
        aura_historia_runtime::ProductListingIngestionDependencies::from_sqs(
            aws_sdk_sqs::Client::new(&aws_client_config),
            ingestion_queue_url_from_env()?,
        );
    let cognito_session_revoker = CognitoUserSessionRevoker::new(
        aws_sdk_cognitoidentityprovider::Client::new(&aws_client_config),
        config.cognito_user_pool_id(),
    );
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listing_history = runtime.public_product_listing_history();
    let listing_dependencies = ProductListingDependencies::from_env(&config.product_listings)?;
    let opensearch_client = listing_dependencies.opensearch.clone();
    let embeddings = Arc::clone(&listing_dependencies.embeddings);

    let get_admin_overview = GetAdminOverviewHandler::new(
        unit_of_work.clone(),
        SqlxAdminOverviewReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let public_overview = runtime.public_overview();
    let create_auction = CreateAuctionHandler::new(
        unit_of_work.clone(),
        SqlxAuctionRepositoryFactory::new(),
        SqlxAuctionEventAppenderFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let get_auction = GetAuctionHandler::new(
        SqlxAuctionDetailsReader::new(pool.clone()),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let search_admin_auctions = SearchAdminAuctionsHandler::new(
        SqlxAdminAuctionSearchReader::new(pool.clone()),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let admin_product_listing_reader = SqlxAdminProductListingReader::new(pool.clone());
    let get_admin_product_listing = GetAdminProductListingHandler::new(
        admin_product_listing_reader.clone(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let search_admin_product_listings = SearchAdminProductListingsHandler::new(
        admin_product_listing_reader,
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let admin_product_listing_mutations = Arc::new(AdminProductListingMutationHandler::new(
        unit_of_work.clone(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        auction_postgres::SqlxAuctionReferenceValidatorFactory::new(),
    ));
    let public_auctions = runtime.public_auctions();
    let address_book = runtime.private_addresses();
    let update_auction = UpdateAuctionHandler::new(
        unit_of_work.clone(),
        SqlxAuctionRepositoryFactory::new(),
        SqlxAuctionEventAppenderFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let create_listing_source = CreateListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let get_listing_source = GetListingSourceHandler::new(
        SqlxListingSourceReaders::new(pool.clone()),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let update_listing_source = UpdateListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxListingSourceRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let delete_listing_source = DeleteListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxListingSourceRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let partner_management = runtime.partner_listing_management();
    let search_listing_sources = SearchListingSourcesHandler::new(
        unit_of_work.clone(),
        SqlxListingSourceSearchReaderFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let get_own_user = runtime.user_profile().get.clone();
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
    let create_party = CreatePartyHandler::new(
        unit_of_work.clone(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let get_party = GetPartyHandler::new(
        unit_of_work.clone(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let update_party = UpdatePartyHandler::new(
        unit_of_work.clone(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let delete_party = DeletePartyHandler::new(
        unit_of_work.clone(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let search_parties = SearchPartiesHandler::new(
        unit_of_work.clone(),
        SqlxPartySearchReaderFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let update_user_profile = runtime.user_profile().update.clone();
    let admin_update_user_profile = UpdateUserProfileHandler::new_admin_only(
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
        SqlxMarketingConsentIntentRepository::new(),
        SqlxUserAdminReaderFactory::new(),
        user_postgres::SqlxUserIdentityDeletionStoreFactory,
    );
    let admin_delete_user = DeleteUserHandler::new_admin_only(
        unit_of_work.clone(),
        SqlxMarketingConsentIntentRepository::new(),
        SqlxUserAdminReaderFactory::new(),
        user_postgres::SqlxUserIdentityDeletionStoreFactory,
    );
    let newsletter_challenges = SqlxNewsletterConfirmationChallengesRepository::new();
    let newsletter_email_sender = SesNewsletterConfirmationEmailSender::new(
        aws_sdk_s3::Client::new(&aws_client_config),
        aws_sdk_sesv2::Client::new(&aws_client_config),
        config.newsletter_confirmation_email().clone(),
    );
    let request_newsletter_subscription = RequestNewsletterSubscriptionHandler::new(
        unit_of_work.clone(),
        newsletter_challenges,
        SqlxNewsletterProfileReader::new(pool.clone()),
        newsletter_email_sender,
        user_service::ports::OsNewsletterConfirmationTokenGenerator,
        user_service::ports::SystemNewsletterConfirmationClock,
    );
    let confirm_newsletter_subscription = ConfirmNewsletterSubscriptionHandler::new(
        unit_of_work.clone(),
        SqlxNewsletterConfirmationChallengesRepository::new(),
        SqlxMarketingConsentIntentRepository::new(),
        user_service::ports::SystemNewsletterConfirmationClock,
    );
    let loops_consent_provider = LoopsMarketingEmailConsentWriter::new(config.loops().clone())
        .map_err(|_| ApiStateError::LoopsClient)?;
    let loops_webhook_target_list_id =
        NewsletterWebhookMailingListId::new(config.loops().newsletter_list_id().to_owned())
            .ok_or(ApiStateError::LoopsWebhookListConfiguration)?;
    let apply_loops_preference_event = ApplyLoopsPreferenceEventHandler::new(
        unit_of_work.clone(),
        SqlxMarketingConsentIntentRepository::new(),
        SqlxLoopsWebhookReceiptRepository::new(),
        loops_consent_provider,
        loops_webhook_target_list_id,
    );
    let submit_partnership_application = SubmitPartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
    );
    let list_own_partnership_applications = ListOwnPartnershipApplicationsHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationReaderFactory::new(),
    );
    let get_own_partnership_application = GetOwnPartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
    );
    let withdraw_partnership_application = WithdrawPartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
    );
    let list_admin_partnership_applications = ListAdminPartnershipApplicationsHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let list_admin_partnerships = ListAdminPartnershipsHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipSearchReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let get_admin_partnership = GetAdminPartnershipHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipDetailsReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let dissolve_partnership = DissolvePartnershipHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let grant_partnership_membership = GrantPartnershipMembershipHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxUserAccountReaderFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let revoke_partnership_membership = RevokePartnershipMembershipHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxUserAccountReaderFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let grant_partnership_listing_source = GrantPartnershipListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxListingSourceGrantRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let revoke_partnership_listing_source = RevokePartnershipListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxListingSourceGrantRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let get_partnership_application = GetPartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let mark_partnership_application_in_review = MarkPartnershipApplicationInReviewHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let approve_partnership_application = ApprovePartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
        SqlxPartyRepositoryFactory::new(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxListingSourceGrantRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
        NotificationCreationCoordinatorFactory::new(
            SqlxNotificationRepositoryFactory::new(),
            InitialExternalDeliveryPlanReaderFactory,
            SqlxNotificationDeliveryIntentRepositoryFactory::new(),
        ),
    );
    let reject_partnership_application = RejectPartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
        SqlxPartyRepositoryFactory::new(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
        NotificationCreationCoordinatorFactory::new(
            SqlxNotificationRepositoryFactory::new(),
            InitialExternalDeliveryPlanReaderFactory,
            SqlxNotificationDeliveryIntentRepositoryFactory::new(),
        ),
    );
    let public_product_listings = runtime.public_product_listings(listing_dependencies);
    let partner_batches = runtime.partner_product_listing_batches();
    let partner_ingestion = runtime.partner_product_listing_ingestion(ingestion_dependencies);
    let submit_product_listing_ingestion = partner_ingestion.submit.clone();
    let authorize_woocommerce_product = AuthorizeProductListingRawCaptureHandler::new(
        unit_of_work.clone(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
    );
    let intake_woocommerce_product = WoocommerceWebhookIntake::new(
        SqlxListingSourceReaders::new(pool.clone()),
        SqlxListingSourceReaders::new(pool.clone()),
        Arc::clone(&submit_product_listing_ingestion),
        authorize_woocommerce_product,
    );
    let access_token_use_case =
        AuthenticateAccessTokenHandler::new(SqlxAccessTokenAuthenticationReader::new(pool.clone()));
    let resolve_cognito_user =
        ResolveCognitoUserHandler::new(SqlxCognitoUserIdentityReader::new(pool.clone()));
    let authenticate_user =
        AuthenticateUserHandler::new(SqlxUserAuthenticationReader::new(pool.clone()));
    let jwks_client = reqwest::Client::builder()
        .connect_timeout(JWKS_CONNECT_TIMEOUT)
        .timeout(JWKS_REQUEST_TIMEOUT)
        .build()
        .map_err(ApiStateError::JwksClient)?;
    let authenticator = compose_authenticator(
        config,
        ReqwestJwksProvider::new(jwks_client),
        resolve_cognito_user,
        AuraAccessTokenAuthenticator::new(access_token_use_case),
        authenticate_user,
    )
    .map_err(ApiStateError::CognitoJwt)?;
    let notifications = runtime.notifications();
    let notifications_state = NotificationsState::new(
        notifications.list.clone(),
        notifications.update_notification_seen.clone(),
        notifications.update_notifications_seen.clone(),
        notifications.update_all_notifications_seen.clone(),
        notifications.delete_notification.clone(),
        notifications.delete_notifications.clone(),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
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
    let partner_product_listing_management_state = state::PartnerProductListingManagementState::new(
        partner_management.get.clone(),
        partner_management.list.clone(),
        partner_management.update.clone(),
        partner_management.withdraw.clone(),
        partner_management.restore.clone(),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let async_partner_product_listings_state = AsyncPartnerProductListingsState::new(
        Arc::clone(&submit_product_listing_ingestion)
            as Arc<
                dyn product_listing_service::use_cases::SubmitPartnerProductListingIngestionUseCase,
            >,
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let partner_product_listings_state = PartnerProductListingsState::new(
        partner_batches.batches.clone(),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let listing_sources_state = ListingSourcesState::new(
        Arc::new(create_listing_source),
        Arc::new(get_listing_source),
        Arc::new(update_listing_source),
        partner_management.sources.clone(),
        Arc::new(search_listing_sources),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    )
    .with_ingestion_configuration(partner_ingestion.put_configuration.clone())
    .with_public_reads(
        Arc::clone(&public_listing_sources.search),
        Arc::clone(&public_listing_sources.get_by_slug),
        config.public_listing_source_read_budget(),
    )
    .with_delete(Arc::new(delete_listing_source));
    use party_postgres::SqlxPartyLocationAccessFactory;
    use party_service::use_cases::party_locations::*;
    let location_admin =
        || CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new());
    let party_locations_state = state::PartyLocationsState {
        list_managers: Arc::new(ListPartyLocationManagersHandler::new(
            unit_of_work.clone(),
            party_postgres::SqlxPartyLocationManagerReaderFactory,
            SqlxPartyLocationAccessFactory,
            location_admin(),
        )),
        discovery: Some(Arc::clone(&runtime.public_party_locations().discover)),
        create: Arc::clone(&runtime.party_locations().create),
        update: Arc::clone(&runtime.party_locations().update),
        lifecycle: Arc::clone(&runtime.party_locations().lifecycle),
        grant: Arc::new(GrantPartyLocationManagementHandler::new(
            unit_of_work.clone(),
            SqlxPartyLocationAccessFactory,
            location_admin(),
        )),
        get: Arc::clone(&runtime.party_locations().get),
        list: Arc::clone(&runtime.party_locations().list),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    use listing_source_postgres::{
        SqlxLocationDefaultReaderFactory, SqlxLocationDefaultRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    };
    use listing_source_service::use_cases::location_default::*;
    let product_listing_delivery_state =
        partner_product_listings::delivery::ProductListingDeliveryState {
            change: runtime.product_listing_delivery().change.clone(),
            get: runtime.product_listing_delivery().get.clone(),
            submit: submit_product_listing_ingestion,
            authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        };
    let shipping = runtime.shipping_management();
    let shipping_state = shipping::ShippingState {
        create: shipping.create.clone(),
        replace: shipping.replace.clone(),
        retire: shipping.retire.clone(),
        restore: shipping.restore.clone(),
        assign_default: shipping.assign_default.clone(),
        clear_default: shipping.clear_default.clone(),
        get: shipping.get.clone(),
        list: shipping.list.clone(),
        get_default: shipping.get_default.clone(),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    let source_location_defaults_state = state::SourceLocationDefaultsState {
        assign: Arc::clone(&runtime.source_location_defaults().assign),
        clear: Arc::clone(&runtime.source_location_defaults().clear),
        change: Arc::clone(&runtime.source_location_defaults().change),
        get: Arc::clone(&runtime.source_location_defaults().get),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    let admin_source_location_defaults_state = state::SourceLocationDefaultsState {
        assign: Arc::new(AssignLocationDefaultHandler::new_admin_only(
            unit_of_work.clone(),
            SqlxLocationDefaultRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        clear: Arc::new(ClearLocationDefaultHandler::new_admin_only(
            unit_of_work.clone(),
            SqlxLocationDefaultRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        change: Arc::new(ChangeLocationDefaultApplicabilityHandler::new_admin_only(
            unit_of_work.clone(),
            SqlxLocationDefaultRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        get: Arc::new(GetLocationDefaultHandler::new_admin_only(
            unit_of_work.clone(),
            SqlxLocationDefaultReaderFactory,
            SqlxSourceLocationAccessFactory,
        )),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    let parties_state = PartiesState::new(
        Arc::new(create_party),
        Arc::new(get_party),
        Arc::new(search_parties),
        Arc::new(update_party),
        Arc::new(delete_party),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let users_state = UsersState {
        get_own_user,
        admin_get_user: Arc::new(admin_get_user),
        search_users: Arc::new(search_users),
        update_user_profile,
        admin_update_user_profile: Arc::new(admin_update_user_profile),
        change_user_role: Arc::new(change_user_role),
        change_user_tier: Arc::new(change_user_tier),
        delete_user: Arc::new(delete_user),
        admin_delete_user: Arc::new(admin_delete_user),
        suspend_user: Arc::new(SuspendUserHandler::new(
            unit_of_work.clone(),
            SqlxUserRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
        )),
        unsuspend_user: Arc::new(UnsuspendUserHandler::new(
            unit_of_work.clone(),
            SqlxUserRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
        )),
        revoke_user_sessions: Arc::new(RevokeUserSessionsHandler::new(
            unit_of_work.clone(),
            SqlxUserAdminReaderFactory::new(),
            SqlxUserCognitoIdentityRegistryFactory::new(),
            cognito_session_revoker,
        )),
        create_access_token: Arc::new(CreateAccessTokenHandler::new(
            unit_of_work.clone(),
            SqlxAccessTokenRepositoryFactory::new(),
        )),
        list_access_tokens: Arc::new(ListAccessTokensHandler::new(
            SqlxAccessTokenListReader::new(pool.clone()),
        )),
        admin_list_access_tokens: Arc::new(ListAdminAccessTokensHandler::new(
            unit_of_work.clone(),
            SqlxAdminAccessTokenListReaderFactory::new(),
            SqlxUserAdminReaderFactory::new(),
            SqlxUserAccountReaderFactory::new(),
        )),
        get_access_token: Arc::new(GetAccessTokenHandler::new(
            SqlxAccessTokenDetailsReader::new(pool.clone()),
        )),
        update_access_token: Arc::new(UpdateAccessTokenHandler::new(
            unit_of_work.clone(),
            SqlxAccessTokenRepositoryFactory::new(),
        )),
        delete_access_token: Arc::new(DeleteAccessTokenHandler::new(
            unit_of_work.clone(),
            SqlxAccessTokenRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
        )),
        admin_delete_access_token: Arc::new(DeleteAccessTokenHandler::new_admin_only(
            unit_of_work.clone(),
            SqlxAccessTokenRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
        )),
        admin_delete_access_tokens: Arc::new(DeleteAccessTokensHandler::new(
            unit_of_work.clone(),
            SqlxAccessTokenRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
            SqlxUserAccountReaderFactory::new(),
        )),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    let saved_searches = runtime.saved_searches(Arc::clone(&embeddings));
    let search_filters_state = SearchFiltersState::new(
        Arc::clone(&saved_searches.list),
        Arc::clone(&saved_searches.create),
        Arc::clone(&saved_searches.get),
        Arc::clone(&saved_searches.update),
        Arc::clone(&saved_searches.delete),
        Arc::clone(&saved_searches.matches),
        Arc::clone(&saved_searches.feedback),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let watchlist = runtime.watchlist();
    let watchlist_state = WatchlistState {
        list_watchlist: Arc::clone(&watchlist.list),
        watch_product: Arc::clone(&watchlist.add),
        update_watchlist_product: Arc::clone(&watchlist.update),
        unwatch_product: Arc::clone(&watchlist.remove),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    let oauth_state = OAuthState {
        resource_policy: config.oauth_resource_policy(),
        create_client: Arc::new(CreateOAuthClientHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        )),
        list_clients: Arc::new(ListOAuthClientsHandler::new(
            SqlxOAuthClientListReader::new(pool.clone()),
            CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        )),
        get_client: Arc::new(GetOAuthClientHandler::new(
            SqlxOAuthClientDetailsReader::new(pool.clone()),
            CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        )),
        get_consent_client: Arc::new(GetOAuthConsentClientHandler::new(
            SqlxOAuthClientDetailsReader::new(pool.clone()),
        )),
        update_client: Arc::new(UpdateOAuthClientHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        )),
        delete_client: Arc::new(DeleteOAuthClientHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        )),
        authorize: Arc::new(AuthorizeHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            SqlxAuthorizationCodeRepositoryFactory::new(),
            config.oauth_resource_policy(),
        )),
        token_by_authorization_code: Arc::new(TokenByAuthorizationCodeHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            SqlxAuthorizationCodeRepositoryFactory::new(),
            SqlxThirdPartyExchangeCodeRepositoryFactory::new(),
            SqlxAccessTokenRepositoryFactory::new(),
            config.oauth_resource_policy(),
        )),
        token_by_third_party_code: Arc::new(TokenByThirdPartyCodeHandler::new(
            unit_of_work.clone(),
            SqlxThirdPartyExchangeCodeRepositoryFactory::new(),
        )),
        revoke: Arc::new(RevokeTokenHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            SqlxAccessTokenRepositoryFactory::new(),
        )),
        introspect: Arc::new(IntrospectTokenHandler::new(
            SqlxOAuthClientAuthenticationReader::new(pool.clone()),
            SqlxAccessTokenAuthenticationReader::new(pool.clone()),
        )),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };

    let partnership_state = PartnershipApplicationsState::new(
        Arc::new(submit_partnership_application),
        Arc::new(list_own_partnership_applications),
        Arc::new(get_own_partnership_application),
        Arc::new(withdraw_partnership_application),
        Arc::new(list_admin_partnership_applications),
        Arc::new(get_partnership_application),
        Arc::new(mark_partnership_application_in_review),
        Arc::new(approve_partnership_application),
        Arc::new(reject_partnership_application),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let partnerships_state = PartnershipsState::new(
        Arc::new(list_admin_partnerships),
        Arc::new(get_admin_partnership),
        Arc::new(grant_partnership_membership),
        Arc::new(revoke_partnership_membership),
        Arc::new(grant_partnership_listing_source),
        Arc::new(revoke_partnership_listing_source),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    )
    .with_dissolve(Arc::new(dissolve_partnership));

    let readiness = Arc::new(RuntimeReadiness {
        postgres: pool,
        opensearch: opensearch_client.clone(),
    });

    let user_addresses_state = state::UserAddressesState {
        commands: address_book.commands.clone(),
        queries: address_book.queries.clone(),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    Ok(AppState::new()
        .with_admin_product_listings(state::AdminProductListingsState::new(
            Arc::new(get_admin_product_listing),
            Arc::new(search_admin_product_listings),
            admin_product_listing_mutations.clone(),
            admin_product_listing_mutations.clone(),
            admin_product_listing_mutations,
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_auctions(AuctionsState::new(
            Arc::new(create_auction),
            Arc::new(get_auction),
            Arc::new(update_auction),
            Arc::new(search_admin_auctions),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_public_auctions(PublicAuctionsState::new(
            Arc::clone(&public_auctions.get),
            Arc::clone(&public_auctions.list),
            Arc::clone(&public_auctions.catalogue),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_admin_overview(AdminOverviewState::new(
            Arc::new(get_admin_overview),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_public_overview(PublicOverviewState::new(Arc::clone(&public_overview.get)))
        .with_parties(parties_state)
        .with_party_locations(party_locations_state)
        .with_shipping(shipping_state)
        .with_product_listing_delivery(product_listing_delivery_state)
        .with_source_location_defaults(source_location_defaults_state)
        .with_admin_source_location_defaults(admin_source_location_defaults_state)
        .with_users(users_state)
        .with_user_addresses(user_addresses_state)
        .with_watchlist(watchlist_state)
        .with_partnership_applications(partnership_state)
        .with_partnerships(partnerships_state)
        .with_products(
            ProductListingsState::new(
                Arc::clone(&public_product_listings.get),
                Arc::clone(&public_product_listings.similar),
                Arc::clone(&public_product_listings.search),
                Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
            )
            .with_product_listing_history(product_listing_history.get.clone()),
        )
        .with_partner_product_listings(partner_product_listings_state)
        .with_partner_product_listing_management(partner_product_listing_management_state)
        .with_async_partner_product_listings(async_partner_product_listings_state)
        .with_listing_sources(listing_sources_state)
        .with_webhooks(WebhooksState::new(
            Arc::new(intake_woocommerce_product),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_loops_webhooks(LoopsWebhooksState::new(
            config.loops_webhook_signing_secret().to_owned(),
            Arc::new(LoopsNewsletterWebhookVerifier),
            Arc::new(apply_loops_preference_event),
        ))
        .with_oauth(oauth_state)
        .with_search_filters(search_filters_state)
        .with_notifications(notifications_state)
        .with_billing(billing_state)
        .with_newsletter(NewsletterState::new(
            Arc::new(request_newsletter_subscription),
            Arc::new(confirm_newsletter_subscription),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_readiness(readiness))
}

fn ingestion_queue_url_from_env() -> Result<String, ApiStateError> {
    let url = std::env::var(PRODUCT_LISTING_INGESTION_QUEUE_URL_ENV).map_err(|_| {
        ApiStateError::MissingEnv {
            name: PRODUCT_LISTING_INGESTION_QUEUE_URL_ENV,
        }
    })?;
    if !valid_ingestion_queue_url(&url) {
        return Err(ApiStateError::InvalidIngestionQueueUrl);
    }
    Ok(url)
}

fn valid_ingestion_queue_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.query().is_none()
            && url.fragment().is_none()
            && url
                .path_segments()
                .and_then(|mut segments| segments.next_back())
                .is_some_and(|name| !name.is_empty() && name.ends_with(".fifo"))
    })
}

fn compose_authenticator<P, R, A, U>(
    config: &ApiConfig,
    jwks_provider: P,
    resolve_cognito_user: R,
    access_token_authenticator: A,
    authenticate_user: U,
) -> Result<Arc<dyn TokenAuthenticator>, AuthError>
where
    P: JwksProvider + 'static,
    R: user_service::use_cases::ResolveCognitoUserUseCase + 'static,
    A: TokenAuthenticator + 'static,
    U: user_service::use_cases::AuthenticateUserUseCase + 'static,
{
    let cognito_jwt = CognitoJwtAuthenticator::new(
        config.cognito_jwt().clone(),
        jwks_provider,
        resolve_cognito_user,
    )?;
    Ok(Arc::new(UserAuthenticationAuthenticator::new(
        ApiAuthService::new(cognito_jwt, access_token_authenticator),
        authenticate_user,
    )))
}

#[derive(thiserror::Error, Debug)]
pub enum ApiStateError {
    #[error("failed to initialize Stripe billing client")]
    StripeBilling(#[source] billing_stripe::StripeBillingClientInitError),
    #[error(transparent)]
    Runtime(#[from] aura_historia_runtime::RuntimeError),
    #[error(transparent)]
    Config(#[from] ApiConfigError),
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("PRODUCT_LISTING_INGESTION_QUEUE_URL must be an HTTP(S) FIFO queue URL")]
    InvalidIngestionQueueUrl,
    #[error(transparent)]
    ListingProviders(#[from] aura_historia_runtime::ProductListingProviderError),
    #[error("failed to configure Cognito JWT authentication: {0}")]
    CognitoJwt(AuthError),
    #[error("failed to build JWKS HTTP client: {0}")]
    JwksClient(reqwest::Error),
    #[error("failed to configure Loops newsletter HTTP client")]
    LoopsClient,
    #[error("invalid Loops webhook target-list configuration")]
    LoopsWebhookListConfiguration,
}

fn log_product_listing_search_cache_config(config: &ApiConfig) {
    let fx = config.product_listings.fx_cache();
    let source = config.product_listings.source_cache();
    info!(
        fx_cache_enabled = fx.enabled(),
        fx_cache_max_entries = fx.max_entries(),
        fx_latest_ttl_seconds = fx.latest_selection_ttl().as_secs(),
        source_cache_enabled = source.enabled(),
        source_cache_max_entries = source.max_entries(),
        source_cache_max_accounted_bytes = source.max_accounted_bytes(),
        source_cache_ttl_seconds = source.ttl().as_secs(),
        parallel_enrichment_enabled = config.product_listings.read_execution_policy()
            == ProductListingSearchReadExecutionPolicy::Concurrent,
        "configured public product-listing search cache policy"
    );
}

pub async fn run_until_shutdown<S>(config: ApiConfig, shutdown: S) -> Result<(), ApiRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    log_product_listing_search_cache_config(&config);
    let state = app_state_from_config(&config)
        .await
        .map_err(ApiRunError::State)?;
    let listener = TcpListener::bind(config.bind_addr())
        .await
        .map_err(ApiRunError::Bind)?;
    serve(
        app_with_request_timeout(state, crate::transport::NATIVE_REQUEST_TIMEOUT),
        listener,
        shutdown,
    )
    .await
}

pub async fn serve<S>(app: Router, listener: TcpListener, shutdown: S) -> Result<(), ApiRunError>
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

#[cfg(test)]
mod readiness_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn valid_api_config_values() -> HashMap<&'static str, String> {
        HashMap::from([
            (COGNITO_ISSUER_ENV, "https://issuer.test".to_owned()),
            (
                COGNITO_JWKS_URL_ENV,
                "https://issuer.test/jwks.json".to_owned(),
            ),
            (COGNITO_APP_CLIENT_IDS_ENV, "api-client".to_owned()),
            (COGNITO_USER_POOL_ID_ENV, "test-pool".to_owned()),
            (STRIPE_API_KEY_ENV, "test-stripe-key".to_owned()),
            (
                STRIPE_CHECKOUT_SUCCESS_URL_ENV,
                "https://client.test/success".to_owned(),
            ),
            (
                STRIPE_CHECKOUT_CANCEL_URL_ENV,
                "https://client.test/cancel".to_owned(),
            ),
            (
                STRIPE_PORTAL_RETURN_URL_ENV,
                "https://client.test/billing".to_owned(),
            ),
            (STRIPE_PRO_MONTHLY_PRICE_ID_ENV, "pro-monthly".to_owned()),
            (STRIPE_PRO_YEARLY_PRICE_ID_ENV, "pro-yearly".to_owned()),
            (
                STRIPE_ULTIMATE_MONTHLY_PRICE_ID_ENV,
                "ultimate-monthly".to_owned(),
            ),
            (
                STRIPE_ULTIMATE_YEARLY_PRICE_ID_ENV,
                "ultimate-yearly".to_owned(),
            ),
            (LOOPS_API_KEY_ENV, "test-loops-key".to_owned()),
            (
                LOOPS_NEWSLETTER_LIST_ID_ENV,
                "test-newsletter-list".to_owned(),
            ),
            (
                LOOPS_WEBHOOK_SIGNING_SECRET_ENV,
                "whsec_dGVzdC1sb29wcy13ZWJob29r".to_owned(),
            ),
            (
                S3_BUCKET_NAME_TEMPLATES_ENV,
                "test-mail-templates".to_owned(),
            ),
            (
                NEWSLETTER_CONFIRMATION_EMAIL_FROM_ENV,
                "Aura Historia <newsletter@example.test>".to_owned(),
            ),
            (
                NEWSLETTER_CONFIRMATION_EMAIL_REPLY_TO_ENV,
                "contact@example.test".to_owned(),
            ),
            (
                NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET_ENV,
                "aura-historia-ephemeral-email".to_owned(),
            ),
            (
                OAUTH_ISSUER_URL_ENV,
                "https://api.aura-historia.com".to_owned(),
            ),
            (STAGE_ENV, "ephemeral".to_owned()),
            (COMMIT_SHA_ENV, "test-commit".to_owned()),
            (MAIL_TEMPLATE_PREFIX_ENV, "mail/test-commit/".to_owned()),
            (
                NEWSLETTER_CONFIRMATION_FRONTEND_ORIGIN_ENV,
                "http://127.0.0.1:3000".to_owned(),
            ),
        ])
    }

    #[test]
    fn should_select_only_exact_oauth_issuers_and_require_explicit_configuration() {
        for policy in [
            oauth_core::resource::AuraResourcePolicy::production(),
            oauth_core::resource::AuraResourcePolicy::stage(),
        ] {
            let mut values = valid_api_config_values();
            values.insert(OAUTH_ISSUER_URL_ENV, policy.issuer().to_owned());
            let config = ApiConfig::from_getter(|name| values.get(name).cloned()).unwrap();
            assert_eq!(policy, config.oauth_resource_policy());
        }
        for issuer in [
            "https://foreign.example",
            "https://api.aura-historia.com/",
            "https://mcp.aura-historia.com/v1",
        ] {
            let mut values = valid_api_config_values();
            values.insert(OAUTH_ISSUER_URL_ENV, issuer.to_owned());
            assert!(matches!(
                ApiConfig::from_getter(|name| values.get(name).cloned()),
                Err(ApiConfigError::InvalidOAuthIssuer(_))
            ));
        }
        let mut values = valid_api_config_values();
        values.remove(OAUTH_ISSUER_URL_ENV);
        assert!(matches!(
            ApiConfig::from_getter(|name| values.get(name).cloned()),
            Err(ApiConfigError::MissingRequiredConfig {
                name: OAUTH_ISSUER_URL_ENV
            })
        ));
    }

    #[test]
    fn should_require_loops_configuration_without_zoho_configuration_and_default_base_url() {
        let values = valid_api_config_values();
        assert!(!values.keys().any(|name| name.starts_with("ZOHO_")));
        let config = ApiConfig::from_getter(|name| values.get(name).cloned())
            .unwrap_or_else(|error| panic!("valid API config rejected: {error}"));
        let expected_loops = LoopsNewsletterConfig::new(
            "test-loops-key".into(),
            "test-newsletter-list".into(),
            DEFAULT_LOOPS_API_BASE_URL.into(),
        )
        .expect("expected Loops config");

        assert!(expected_loops == config.loops);
    }

    #[test]
    fn should_require_newsletter_confirmation_email_configuration_set() {
        let mut missing = valid_api_config_values();
        missing.remove(NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET_ENV);
        assert!(matches!(
            ApiConfig::from_getter(|name| missing.get(name).cloned()),
            Err(ApiConfigError::MissingRequiredConfig {
                name: NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET_ENV
            })
        ));

        for value in ["", "   ", "\t\n"] {
            let mut blank = valid_api_config_values();
            blank.insert(
                NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET_ENV,
                value.to_owned(),
            );
            assert!(matches!(
                ApiConfig::from_getter(|name| blank.get(name).cloned()),
                Err(ApiConfigError::MissingRequiredConfig {
                    name: NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET_ENV
                })
            ));
        }
    }

    #[test]
    fn should_validate_newsletter_confirmation_email_configuration_set_without_fallback() {
        for value in [
            " aura-historia-ephemeral-email",
            "aura-historia-ephemeral-email ",
            "invalid.name",
            "invalid/name",
            "café",
            &"a".repeat(65),
        ] {
            let mut invalid = valid_api_config_values();
            invalid.insert(
                NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET_ENV,
                value.to_owned(),
            );
            let Err(error) = ApiConfig::from_getter(|name| invalid.get(name).cloned()) else {
                panic!("invalid SES configuration set unexpectedly accepted");
            };
            assert!(matches!(
                error,
                ApiConfigError::NewsletterConfirmationEmailConfig(
                    user_email_aws::NewsletterConfirmationEmailConfigError::InvalidConfiguration
                )
            ));
            assert!(!error.to_string().contains(value));
            assert!(!format!("{error:?}").contains(value));
        }

        for value in ["Custom_Set-09", &"a".repeat(64)] {
            let mut values = valid_api_config_values();
            values.insert(
                NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET_ENV,
                value.to_owned(),
            );
            let config = ApiConfig::from_getter(|name| values.get(name).cloned())
                .unwrap_or_else(|error| panic!("valid API config rejected: {error}"));
            let expected = NewsletterConfirmationEmailConfig::new(
                "test-mail-templates",
                "Aura Historia <newsletter@example.test>",
                "contact@example.test",
                value,
                "ephemeral",
                "mail/test-commit/",
                "http://127.0.0.1:3000",
            )
            .unwrap_or_else(|_| panic!("valid newsletter email config rejected"));
            assert!(config.newsletter_confirmation_email == expected);
        }
    }

    #[test]
    fn should_validate_newsletter_confirmation_origin_for_each_deployment_stage() {
        for (stage, origin) in [
            ("prod", "https://aura-historia.com"),
            ("dev", "https://stage.aura-historia.com"),
            ("ephemeral", "http://127.0.0.1:3000"),
        ] {
            let mut values = valid_api_config_values();
            values.insert(STAGE_ENV, stage.to_owned());
            values.insert(
                NEWSLETTER_CONFIRMATION_FRONTEND_ORIGIN_ENV,
                origin.to_owned(),
            );
            assert!(ApiConfig::from_getter(|name| values.get(name).cloned()).is_ok());
        }

        let mut untrusted = valid_api_config_values();
        untrusted.insert(STAGE_ENV, "prod".to_owned());
        untrusted.insert(
            NEWSLETTER_CONFIRMATION_FRONTEND_ORIGIN_ENV,
            "https://attacker.example".to_owned(),
        );
        assert!(matches!(
            ApiConfig::from_getter(|name| untrusted.get(name).cloned()),
            Err(ApiConfigError::NewsletterConfirmationEmailConfig(_))
        ));
    }

    #[test]
    fn should_reject_missing_or_blank_loops_key_and_list_configuration() {
        for name in [LOOPS_API_KEY_ENV, LOOPS_NEWSLETTER_LIST_ID_ENV] {
            let mut missing = valid_api_config_values();
            missing.remove(name);
            assert!(matches!(
                ApiConfig::from_getter(|candidate| missing.get(candidate).cloned()),
                Err(ApiConfigError::LoopsConfig(_))
            ));

            let mut blank = valid_api_config_values();
            blank.insert(name, "   ".to_owned());
            assert!(matches!(
                ApiConfig::from_getter(|candidate| blank.get(candidate).cloned()),
                Err(ApiConfigError::LoopsConfig(_))
            ));
        }
    }

    #[test]
    fn should_validate_required_loops_webhook_signing_secret() {
        let mut missing = valid_api_config_values();
        missing.remove(LOOPS_WEBHOOK_SIGNING_SECRET_ENV);
        assert!(matches!(
            ApiConfig::from_getter(|name| missing.get(name).cloned()),
            Err(ApiConfigError::MissingRequiredConfig {
                name: LOOPS_WEBHOOK_SIGNING_SECRET_ENV
            })
        ));

        for value in ["", "   ", "whsec_not-base64!"] {
            let mut invalid = valid_api_config_values();
            invalid.insert(LOOPS_WEBHOOK_SIGNING_SECRET_ENV, value.to_owned());
            let result = ApiConfig::from_getter(|name| invalid.get(name).cloned());
            if value.trim().is_empty() {
                assert!(matches!(
                    result,
                    Err(ApiConfigError::MissingRequiredConfig { .. })
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(ApiConfigError::InvalidLoopsWebhookSigningSecret)
                ));
            }
        }
    }

    #[test]
    fn should_not_reveal_loops_webhook_signing_secret_in_configuration_errors() {
        let sentinel = "whsec-secret-sentinel";
        let mut values = valid_api_config_values();
        values.insert(LOOPS_WEBHOOK_SIGNING_SECRET_ENV, sentinel.to_owned());
        let Err(error) = ApiConfig::from_getter(|name| values.get(name).cloned()) else {
            panic!("invalid Loops webhook secret unexpectedly accepted");
        };

        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
    }

    #[test]
    fn should_not_reveal_loops_secret_in_api_configuration_errors() {
        let sentinel = "loops-api-key-secret-sentinel";
        let mut values = valid_api_config_values();
        values.insert(LOOPS_API_KEY_ENV, format!("{sentinel}\ninvalid"));
        let Err(error) = ApiConfig::from_getter(|name| values.get(name).cloned()) else {
            panic!("invalid Loops API key unexpectedly accepted");
        };

        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
    }

    #[test]
    fn should_require_valid_fifo_ingestion_queue_url_without_leaking_url() {
        assert!(valid_ingestion_queue_url(
            "https://sqs.eu-west-1.amazonaws.com/123456789012/product-listing-ingestion.fifo"
        ));
        for value in [
            "",
            "https://sqs.eu-west-1.amazonaws.com/123456789012/standard",
            "https://sqs.eu-west-1.amazonaws.com/123456789012/ingestion.fifo?token=secret",
            "ftp://sqs.example.test/ingestion.fifo",
        ] {
            assert!(!valid_ingestion_queue_url(value));
        }
        assert!(
            !ApiStateError::InvalidIngestionQueueUrl
                .to_string()
                .contains("secret")
        );
    }

    #[test]
    fn should_default_and_apply_bounded_public_listing_source_read_configuration() {
        let mut absent = |_| None;
        assert_eq!(
            DEFAULT_PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT as usize,
            optional_bounded_usize_config(
                &mut absent,
                PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT_ENV,
                DEFAULT_PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT,
                1,
                64,
            )
            .unwrap_or_default(),
        );

        let mut configured = |name| match name {
            PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT_ENV => Some("8".to_owned()),
            PUBLIC_LISTING_SOURCE_READ_REQUEST_TIMEOUT_MS_ENV => Some("250".to_owned()),
            _ => None,
        };
        assert_eq!(
            8,
            optional_bounded_usize_config(
                &mut configured,
                PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT_ENV,
                DEFAULT_PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT,
                1,
                64,
            )
            .unwrap_or_default(),
        );
        assert_eq!(
            250,
            optional_bounded_u64_config(
                &mut configured,
                PUBLIC_LISTING_SOURCE_READ_REQUEST_TIMEOUT_MS_ENV,
                DEFAULT_PUBLIC_LISTING_SOURCE_READ_REQUEST_TIMEOUT_MS,
                1,
                5_000,
            )
            .unwrap_or_default(),
        );
    }

    #[test]
    fn should_reject_invalid_or_out_of_range_public_listing_source_read_configuration() {
        for (name, value, min, max) in [
            (PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT_ENV, "0", 1, 64),
            (PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT_ENV, "65", 1, 64),
            (
                PUBLIC_LISTING_SOURCE_READ_REQUEST_TIMEOUT_MS_ENV,
                "0",
                1,
                5_000,
            ),
            (
                PUBLIC_LISTING_SOURCE_READ_REQUEST_TIMEOUT_MS_ENV,
                "5001",
                1,
                5_000,
            ),
        ] {
            let mut get = |candidate| (candidate == name).then(|| value.to_owned());
            assert!(matches!(
                optional_bounded_u64_config(&mut get, name, 1, min, max),
                Err(ApiConfigError::OutOfRangeIntegerConfig { .. })
            ));
        }
        let mut get = |name| {
            (name == PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT_ENV).then(|| "many".to_owned())
        };
        assert!(matches!(
            optional_bounded_usize_config(
                &mut get,
                PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT_ENV,
                DEFAULT_PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT,
                1,
                64,
            ),
            Err(ApiConfigError::InvalidIntegerConfig { .. })
        ));
    }
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
