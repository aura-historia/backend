use aws_config::BehaviorVersion;
use cloudwatch_log_retention_lambda::{LOG_RETENTION_DAYS, handler};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

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
