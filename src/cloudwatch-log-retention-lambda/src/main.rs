use aws_config::BehaviorVersion;
use cloudwatch_log_retention_lambda::{LOG_RETENTION_DAYS, handler};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{log_cold_start, log_invocation_start, logging_config_from_env};
use platform_observability::init;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let client = aws_sdk_cloudwatchlogs::Client::new(&aws_config);

    log_cold_start("cloudwatch-log-retention-lambda", initialization_started_at);
    tracing::debug!(retention_days = LOG_RETENTION_DAYS, "Lambda initialized.");

    run(service_fn(|event: LambdaEvent<serde_json::Value>| async {
        log_invocation_start("cloudwatch-log-retention-lambda", &event.context);
        handler(&client, event).await
    }))
    .await
}
