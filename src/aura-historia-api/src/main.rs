use aura_historia_api::consent_evidence::{
    ConsentEvidenceConfigError, ConsentEvidenceLogGroups, init_logging,
};
use aura_historia_api::{ApiConfig, ApiConfigError, ApiRunError, run_until_shutdown};
use platform_observability::LoggingConfig;
use platform_process::{
    Shutdown, ShutdownError, config::load_file_backed_environment, install_bounded_panic_hook,
};
use std::time::Duration;

/// Shipping queued consent evidence after the drain; within the container's stop timeout.
const EVIDENCE_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

fn main() -> Result<(), MainError> {
    // A handler can panic on provider or persisted data; never print the payload.
    install_bounded_panic_hook("aura-historia-api");
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
                Some(evidence) => Some(
                    evidence
                        .start(cloudwatch_logs_client().await)
                        .await
                        .map_err(MainError::ConsentEvidenceStart)?,
                ),
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

/// Bounded, so a hung delivery cannot stall the shipper while the queue fills.
async fn cloudwatch_logs_client() -> aws_sdk_cloudwatchlogs::Client {
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .timeout_config(
            aws_config::timeout::TimeoutConfig::builder()
                .connect_timeout(Duration::from_secs(3))
                .operation_attempt_timeout(Duration::from_secs(5))
                .operation_timeout(Duration::from_secs(10))
                .build(),
        )
        .load()
        .await;
    aws_sdk_cloudwatchlogs::Client::new(&config)
}

#[derive(thiserror::Error, Debug)]
enum MainError {
    #[error("invalid configuration")]
    Config(#[source] application::error::BoxError),
    #[error(transparent)]
    ConsentEvidence(#[from] ConsentEvidenceConfigError),
    #[error("failed to reach the consent-evidence log groups")]
    ConsentEvidenceStart(#[source] application::error::BoxError),
    #[error(transparent)]
    ApiConfig(#[from] ApiConfigError),
    #[error(transparent)]
    Run(#[from] ApiRunError),
    #[error(transparent)]
    Shutdown(#[from] ShutdownError),
    #[error("failed to build API async runtime")]
    Runtime(#[source] std::io::Error),
}
