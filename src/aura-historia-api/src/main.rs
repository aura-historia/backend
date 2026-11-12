use aura_historia_api::{
    ApiConfig, ApiConfigError, ApiRunError, ApiStateError, lambda_app_from_config_and_runtime,
    run_until_shutdown,
};
use aura_historia_runtime::Runtime;
use platform_lambda_bootstrap::{
    LazyComposition, log_cold_start, materialize_google_application_credentials_from_env,
};
use platform_observability::{LoggingConfig, init};
use std::sync::Arc;
use std::time::Instant;

const LAMBDA_COMPONENT: &str = "aura-historia-api";

fn main() -> Result<(), MainError> {
    let initialization_started_at = Instant::now();

    let is_lambda = std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some();
    if is_lambda {
        materialize_google_application_credentials_from_env()
            .map_err(|source| MainError::GoogleAdcCredentials(Box::new(source)))?;
    }
    init(LoggingConfig::from_env());

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(MainError::Runtime)?
        .block_on(async move {
            if is_lambda {
                let config = Arc::new(ApiConfig::from_env()?);
                let runtime = Arc::new(Runtime::from_env().await?);
                let routers = Arc::new(LazyComposition::new());

                log_cold_start(LAMBDA_COMPONENT, initialization_started_at);
                aura_historia_api::lambda::run(move || {
                    let config = Arc::clone(&config);
                    let runtime = Arc::clone(&runtime);
                    let routers = Arc::clone(&routers);
                    async move {
                        routers
                            .get_or_try_build(|| async {
                                lambda_app_from_config_and_runtime(&config, &runtime)
                                    .await
                                    .map_err(MainError::from)
                            })
                            .await
                    }
                })
                .await?;
            } else {
                let config = ApiConfig::from_env()?;
                run_until_shutdown(config, shutdown_signal()).await?;
            }

            Ok(())
        })
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for shutdown signal");
    }
}

#[derive(thiserror::Error, Debug)]
enum MainError {
    #[error(transparent)]
    Config(#[from] ApiConfigError),
    #[error(transparent)]
    Run(#[from] ApiRunError),

    #[error(transparent)]
    Composition(#[from] aura_historia_runtime::RuntimeError),
    #[error(transparent)]
    State(#[from] ApiStateError),
    #[error("failed to build API async runtime")]
    Runtime(#[source] std::io::Error),
    #[error("failed to prepare Google application credentials")]
    GoogleAdcCredentials(#[source] application::error::BoxError),
    #[error(transparent)]
    Lambda(#[from] lambda_http::Error),
}
