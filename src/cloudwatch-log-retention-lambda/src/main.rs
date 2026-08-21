use aws_config::BehaviorVersion;
use cloudwatch_log_retention_lambda::{LOG_RETENTION_DAYS, handler};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_observability::{LogLevel, LoggingConfig, init};
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    init(logging_config_from_env());

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let client = aws_sdk_cloudwatchlogs::Client::new(&aws_config);

    debug!(retention_days = LOG_RETENTION_DAYS, "Lambda initialized.");

    run(service_fn(|event: LambdaEvent<serde_json::Value>| async {
        handler(&client, event).await
    }))
    .await
}

fn logging_config_from_env() -> LoggingConfig {
    let level = std::env::var("LOG_LEVEL")
        .ok()
        .as_deref()
        .and_then(LogLevel::parse)
        .unwrap_or_default();
    LoggingConfig::new(level)
}
