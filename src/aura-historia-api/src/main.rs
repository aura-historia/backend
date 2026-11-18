use aura_historia_api::consent_evidence::{
    ConsentEvidenceConfigError, ConsentEvidenceLogGroups, init_logging,
};
use aura_historia_api::{ApiConfig, ApiConfigError, ApiRunError, run_until_shutdown};
use platform_observability::LoggingConfig;
use platform_process::{Shutdown, ShutdownError, config::load_file_backed_environment};
use std::time::Duration;

/// Shipping queued consent evidence after the drain; within the container's stop timeout.
const EVIDENCE_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

fn main() -> Result<(), MainError> {
    load_file_backed_environment().map_err(|source| MainError::Config(Box::new(source)))?;
    let evidence_groups = ConsentEvidenceLogGroups::from_getter(|name| std::env::var(name).ok())?;
    let evidence = init_logging(LoggingConfig::from_env(), evidence_groups);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(MainError::Runtime)?
        .block_on(async move {
            // Registered before startup, so a stop request during composition is not lost.
            let shutdown = Shutdown::register()?;
            let evidence = match evidence {
                Some(evidence) => {
                    let aws =
                        aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                    Some(evidence.start(aws_sdk_cloudwatchlogs::Client::new(&aws)))
                }
                None => None,
            };
            let config = ApiConfig::from_env()?;
            let served = run_until_shutdown(config, shutdown.received()).await;
            if let Some(evidence) = evidence {
                evidence.close(EVIDENCE_CLOSE_TIMEOUT).await;
            }
            served?;
            Ok(())
        })
}

#[derive(thiserror::Error, Debug)]
enum MainError {
    #[error("invalid configuration")]
    Config(#[source] application::error::BoxError),
    #[error(transparent)]
    ConsentEvidence(#[from] ConsentEvidenceConfigError),
    #[error(transparent)]
    ApiConfig(#[from] ApiConfigError),
    #[error(transparent)]
    Run(#[from] ApiRunError),
    #[error(transparent)]
    Shutdown(#[from] ShutdownError),
    #[error("failed to build API async runtime")]
    Runtime(#[source] std::io::Error),
}
