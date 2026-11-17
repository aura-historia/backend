use aura_historia_api::{ApiConfig, ApiConfigError, ApiRunError, run_until_shutdown};
use platform_observability::{LoggingConfig, init_with_directives};
use platform_process::{Shutdown, ShutdownError, config::load_file_backed_environment};
use user_service::use_cases::CONSENT_EVIDENCE_LOG_DIRECTIVE;

fn main() -> Result<(), MainError> {
    load_file_backed_environment().map_err(|source| MainError::Config(Box::new(source)))?;
    init_with_directives(LoggingConfig::from_env(), &[CONSENT_EVIDENCE_LOG_DIRECTIVE]);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(MainError::Runtime)?
        .block_on(async move {
            // Registered before startup, so a stop request during composition is not lost.
            let shutdown = Shutdown::register()?;
            let config = ApiConfig::from_env()?;
            run_until_shutdown(config, shutdown.received()).await?;
            Ok(())
        })
}

#[derive(thiserror::Error, Debug)]
enum MainError {
    #[error("invalid configuration")]
    Config(#[source] application::error::BoxError),
    #[error(transparent)]
    ApiConfig(#[from] ApiConfigError),
    #[error(transparent)]
    Run(#[from] ApiRunError),
    #[error(transparent)]
    Shutdown(#[from] ShutdownError),
    #[error("failed to build API async runtime")]
    Runtime(#[source] std::io::Error),
}
