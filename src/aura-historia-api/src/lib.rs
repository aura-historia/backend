pub mod auth;
pub mod error;
pub mod shops;
pub mod state;

use crate::auth::{
    ApiAuthService, AuraAccessTokenAuthenticator, AuthError, RequestMetadata, TokenAuthenticator,
    TransportPrincipal,
};
use crate::state::{AppState, ShopsState};
use axum::Router;
use axum::routing::get;
use common::postgres::{PostgresConnectError, SqlxUnitOfWork};
use shop_postgres::SqlxShopDetailsReaderFactory;
use shop_service::use_cases::queries::get_shop::GetShopHandler;
use std::future::Future;
use std::net::{AddrParseError, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;
use user_dynamodb::DynamoDbAccessTokenStore;
use user_service::use_cases::AuthenticateAccessTokenHandler;

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
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/api/v1/shops/{shop_id}", get(shops::get_shop::get_shop))
        .with_state(state.shops)
}

async fn health() -> &'static str {
    "ok\n"
}

async fn ready() -> &'static str {
    "ready\n"
}

pub async fn app_state_from_env() -> Result<AppState, ApiStateError> {
    let pool = common::postgres::connect_from_env().await?;
    let get_shop = GetShopHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxShopDetailsReaderFactory::new(),
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
    let access_token_use_case = AuthenticateAccessTokenHandler::new(access_token_store);
    let authenticator = ApiAuthService::new(
        JwtUnavailableAuthenticator,
        AuraAccessTokenAuthenticator::new(access_token_use_case),
    );

    Ok(AppState::new(ShopsState::new(
        Arc::new(get_shop),
        Arc::new(authenticator),
    )))
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
        AppState::new(ShopsState::new(
            Arc::new(RejectGetShopUseCase),
            Arc::new(StaticAuthenticator),
        ))
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
