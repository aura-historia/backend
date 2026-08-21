use aura_historia_api::{ApiConfig, ApiConfigError, ApiRunError, run_until_shutdown};
use platform_observability::{LogLevel, LoggingConfig, init};

#[tokio::main]
async fn main() -> Result<(), MainError> {
    init(logging_config_from_env());
    let config = ApiConfig::from_env()?;
    run_until_shutdown(config, shutdown_signal()).await?;
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
}
