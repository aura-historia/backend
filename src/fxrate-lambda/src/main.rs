use aws_config::BehaviorVersion;
use fxrate::{
    dynamodb::repository::FxRateDynamoDbRepositoryImpl, fxratesapi::FxRatesApiClientImpl,
    service::FxRateServiceImpl,
};
use fxrate_lambda::handler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let fxrates_api_token = std::env::var("FXRATES_API_TOKEN")
        .expect("shouldn't fail loading env-var 'FXRATES_API_TOKEN'");
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);
    let reqwest = reqwest::Client::new();
    let repository = FxRateDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let fxrates_api = FxRatesApiClientImpl::new(&reqwest, &fxrates_api_token);
    let service = FxRateServiceImpl::new(&fxrates_api, &repository);

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<serde_json::Value>| async {
        handler(&service, event).await
    }))
    .await
}
