use aura_historia_api::{
    ApiConfig, ApiConfigError, ApiRunError, ApiStateError, lambda_app_from_config_and_runtime,
    run_until_shutdown,
};
use aura_historia_runtime::RotatingRuntime;
use platform_lambda_bootstrap::{
    VersionedCompositionCache, log_cold_start, materialize_google_application_credentials_from_env,
};
use platform_observability::{LogLevel, LoggingConfig, init};
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
    init(logging_config_from_env());

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(MainError::Runtime)?
        .block_on(async move {
            if is_lambda {
                let config = ApiConfig::from_env()?;
                let runtime = Arc::new(RotatingRuntime::from_env(LAMBDA_COMPONENT).await?);
                let routers = Arc::new(VersionedCompositionCache::new());

                log_cold_start(LAMBDA_COMPONENT, initialization_started_at);
                aura_historia_api::lambda::run(move || {
                    let config = config.clone();
                    let runtime = Arc::clone(&runtime);
                    let routers = Arc::clone(&routers);
                    async move {
                        let runtime = runtime.current().await?;
                        let router = routers
                            .get_or_try_build(runtime.version_id(), || async {
                                lambda_app_from_config_and_runtime(&config, runtime.value()).await
                            })
                            .await?;
                        Ok::<_, MainError>(router.value().clone())
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

fn logging_config_from_env() -> LoggingConfig {
    let level = std::env::var("LOG_LEVEL")
        .ok()
        .as_deref()
        .and_then(LogLevel::parse)
        .unwrap_or_default();
    LoggingConfig::new(level)
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
