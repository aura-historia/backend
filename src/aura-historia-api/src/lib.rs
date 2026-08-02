pub mod auth;
pub mod error;
pub mod partner_applications;
pub mod shops;
pub mod state;
pub mod users;
pub mod watchlist;

use crate::auth::{
    ApiAuthService, AuraAccessTokenAuthenticator, AuthError, RequestMetadata, TokenAuthenticator,
    TransportPrincipal,
};
use crate::state::{AppState, PartnerApplicationsState, ShopsState, UsersState, WatchlistState};
use axum::Router;
use axum::routing::{delete, get, patch, post};
use common::postgres::{PostgresConnectError, SqlxUnitOfWork};
use shop_partner_postgres::{
    SqlxPartnerShopApplicationReaderFactory, SqlxPartnerShopApplicationRepositoryFactory,
};
use shop_partner_service::use_cases::{
    AdminDecidePartnerShopApplicationHandler, AdminGetPartnerShopApplicationHandler,
    AdminListPartnerShopApplicationsHandler, AdminUpdatePartnerShopApplicationHandler,
    CreatePartnerShopApplicationHandler, DeletePartnerShopApplicationHandler,
    GetPartnerShopApplicationHandler, ListPartnerShopApplicationsHandler,
};
use shop_postgres::{
    SqlxPartnerShopReaderFactory, SqlxShopDetailsReaderFactory, SqlxShopRepositoryFactory,
    SqlxShopSearchReaderFactory,
};
use shop_service::ports::{ShopGeocoder, ShopGeocoderError};
use shop_service::use_cases::commands::create_shop::CreateShopHandler;
use shop_service::use_cases::commands::update_shop::UpdateShopHandler;
use shop_service::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopHandler;
use shop_service::use_cases::queries::get_shop::GetShopHandler;
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsHandler;
use shop_service::use_cases::queries::search_shops::SearchShopsHandler;
use std::future::Future;
use std::net::{AddrParseError, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;
use user_dynamodb::DynamoDbAccessTokenStore;
use user_postgres::{
    SqlxUserAccountReaderFactory, SqlxUserAdminReaderFactory, SqlxUserRepositoryFactory,
    SqlxUserSearchReaderFactory,
};
use user_service::use_cases::AuthenticateAccessTokenHandler;
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
    DeleteWatchlistProductHandler, ListWatchlistHandler, UpdateWatchlistProductHandler,
    WatchProductHandler,
};

pub const API_BIND_ADDR_ENV: &str = "AURA_HISTORIA_API_BIND_ADDR";
pub const DYNAMODB_TABLE_NAME_ENV: &str = "DYNAMODB_TABLE_NAME";
const DEFAULT_API_BIND_ADDR: &str = "0.0.0.0:8080";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiConfig {
    bind_addr: SocketAddr,
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

        Ok(Self { bind_addr })
    }

    pub const fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ApiConfigError {
    #[error("invalid {env_name}: {value}", env_name = API_BIND_ADDR_ENV)]
    InvalidBindAddr {
        value: String,
        source: AddrParseError,
    },
}

pub fn app(state: AppState) -> Router {
    let health_routes = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready));
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

    routes
}

async fn health() -> &'static str {
    "ok\n"
}

async fn ready() -> &'static str {
    "ready\n"
}

pub async fn app_state_from_env() -> Result<AppState, ApiStateError> {
    let pool = common::postgres::connect_from_env().await?;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let get_shop = GetShopHandler::new(unit_of_work.clone(), SqlxShopDetailsReaderFactory::new());
    let search_shops =
        SearchShopsHandler::new(unit_of_work.clone(), SqlxShopSearchReaderFactory::new());
    let check_user_admin =
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new());
    let check_user_partner_shop =
        CheckUserPartnerShopHandler::new(unit_of_work.clone(), SqlxPartnerShopReaderFactory::new());
    let create_shop = CreateShopHandler::new(
        unit_of_work.clone(),
        SqlxShopRepositoryFactory::new(),
        UnavailableShopGeocoder,
        check_user_admin,
    );
    let update_shop = UpdateShopHandler::new(
        unit_of_work.clone(),
        SqlxShopRepositoryFactory::new(),
        UnavailableShopGeocoder,
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        check_user_partner_shop,
    );
    let list_user_partner_shops =
        ListUserPartnerShopsHandler::new(unit_of_work.clone(), SqlxPartnerShopReaderFactory::new());
    let get_own_user =
        GetOwnUserHandler::new(unit_of_work.clone(), SqlxUserAccountReaderFactory::new());
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
    );
    let delete_user = DeleteUserHandler::new(
        unit_of_work.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let list_watchlist =
        ListWatchlistHandler::new(unit_of_work.clone(), SqlxWatchlistReaderFactory);
    let watch_product =
        WatchProductHandler::new(unit_of_work.clone(), SqlxWatchlistRepositoryFactory);
    let update_watchlist_product =
        UpdateWatchlistProductHandler::new(unit_of_work.clone(), SqlxWatchlistRepositoryFactory);
    let delete_watchlist_product =
        DeleteWatchlistProductHandler::new(unit_of_work.clone(), SqlxWatchlistRepositoryFactory);
    let create_partner_application = CreatePartnerShopApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
        SqlxShopRepositoryFactory::new(),
        UnavailableShopGeocoder,
    );
    let list_partner_applications = ListPartnerShopApplicationsHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationReaderFactory::new(),
    );
    let get_partner_application = GetPartnerShopApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
    );
    let delete_partner_application = DeletePartnerShopApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
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
    let admin_decide_partner_application = AdminDecidePartnerShopApplicationHandler::new(
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
    let table_name = Box::leak(table_name.into_boxed_str());
    let access_token_store = DynamoDbAccessTokenStore::new(dynamodb_client, table_name);
    let access_token_use_case = AuthenticateAccessTokenHandler::new(access_token_store.clone());
    let authenticator = Arc::new(ApiAuthService::new(
        JwtUnavailableAuthenticator,
        AuraAccessTokenAuthenticator::new(access_token_use_case),
    ));
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
    let watchlist_state = WatchlistState {
        list_watchlist: Arc::new(list_watchlist),
        watch_product: Arc::new(watch_product),
        update_watchlist_product: Arc::new(update_watchlist_product),
        delete_watchlist_product: Arc::new(delete_watchlist_product),
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

    Ok(AppState::new(
        ShopsState::new(
            Arc::new(get_shop),
            Arc::new(search_shops),
            Arc::new(create_shop),
            Arc::new(update_shop),
            Arc::new(list_user_partner_shops),
            authenticator,
        ),
        users_state,
        watchlist_state,
        partner_state,
    ))
}

#[derive(Clone, Copy)]
struct UnavailableShopGeocoder;

#[async_trait::async_trait]
impl ShopGeocoder for UnavailableShopGeocoder {
    async fn geocode(
        &self,
        _address: &shop_core::address::StructuredAddress,
    ) -> Result<shop_core::address::GeoAddress, ShopGeocoderError> {
        Err(ShopGeocoderError::TemporarilyUnavailable)
    }
}

struct JwtUnavailableAuthenticator;

#[async_trait::async_trait]
impl TokenAuthenticator for JwtUnavailableAuthenticator {
    async fn authenticate(
        &self,
        _bearer_token: &str,
        _metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        Err(AuthError::TemporarilyUnavailable)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ApiStateError {
    #[error(transparent)]
    Postgres(#[from] PostgresConnectError),
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
}

pub async fn run_until_shutdown<S>(config: ApiConfig, shutdown: S) -> Result<(), ApiRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    let state = app_state_from_env().await.map_err(ApiRunError::State)?;
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
    use crate::auth::{AuthMethod, TransportPrincipal};
    use common::operation_context::CredentialCapability;
    use common::user_id::UserId;
    use http::StatusCode;

    use std::collections::BTreeSet;
    use tokio::sync::oneshot;

    fn env(values: &[(&'static str, &str)]) -> HashMap<&'static str, String> {
        values
            .iter()
            .map(|(key, value)| (*key, (*value).to_owned()))
            .collect()
    }

    #[test]
    fn should_use_default_bind_addr_when_env_missing() -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[]);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!("0.0.0.0:8080".parse::<SocketAddr>()?, config.bind_addr());
        Ok(())
    }

    #[test]
    fn should_read_bind_addr_from_env() -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[(API_BIND_ADDR_ENV, "127.0.0.1:9000")]);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!("127.0.0.1:9000".parse::<SocketAddr>()?, config.bind_addr());
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

    #[tokio::test]
    async fn should_route_health_endpoints() -> Result<(), Box<dyn std::error::Error>> {
        for (path, status_code, body) in [
            ("/health", StatusCode::OK, "ok\n"),
            ("/ready", StatusCode::OK, "ready\n"),
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
                delete_watchlist_product: Arc::new(UnusedUseCase),
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
    impl watchlist_service::use_cases::DeleteWatchlistProductUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: watchlist_service::use_cases::DeleteWatchlistProductCommand,
        ) -> Result<
            watchlist_service::use_cases::DeleteWatchlistProductResult,
            watchlist_service::use_cases::DeleteWatchlistProductError,
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
    impl shop_partner_service::use_cases::DeletePartnerShopApplicationUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _command: shop_partner_service::use_cases::DeletePartnerShopApplicationCommand,
        ) -> Result<(), shop_partner_service::use_cases::DeletePartnerShopApplicationError>
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
