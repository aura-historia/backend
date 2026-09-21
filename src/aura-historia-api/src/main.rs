use aura_historia_api::{
    ApiConfig, ApiConfigError, ApiRunError, ApiStateError, lambda_app_from_env, run_until_shutdown,
};
use platform_lambda_bootstrap::log_cold_start;
use platform_observability::{LogLevel, LoggingConfig, init};
use std::time::Instant;

const LAMBDA_COMPONENT: &str = "aura-historia-api";

#[tokio::main]
async fn main() -> Result<(), MainError> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    if std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some() {
        let app = lambda_app_from_env().await?;
        log_cold_start(LAMBDA_COMPONENT, initialization_started_at);
        aura_historia_api::lambda::run(app).await?;
    } else {
        let config = ApiConfig::from_env()?;
        run_until_shutdown(config, shutdown_signal()).await?;
    }

    Ok(())
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
    State(#[from] ApiStateError),
    #[error(transparent)]
    Lambda(#[from] lambda_http::Error),
}
