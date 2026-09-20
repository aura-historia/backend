use aws_lambda_events::eventbridge::EventBridgeEvent;
use fxrate_fxratesapi::FxRatesApiQuoteProvider;
use fxrate_lambda::handler;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use fxrate_service::CaptureFxRateSnapshotHandler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, log_cold_start, log_invocation_start, logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use serde_json::Value;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let pool = LambdaPostgresConfig::from_env()?
        .into_pool_config()
        .connect()
        .await
        .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
    let token = std::env::var("FXRATES_API_TOKEN")
        .map_err(|_| Error::from("missing required environment variable FXRATES_API_TOKEN"))?;
    let snapshots = CaptureFxRateSnapshotHandler::new(
        FxRatesApiQuoteProvider::new(reqwest::Client::new(), token),
        SqlxUnitOfWork::new(pool),
        SqlxFxRateSnapshotRepositoryFactory::new(),
    );

    log_cold_start("fxrate-lambda", initialization_started_at);
    run(service_fn(
        |event: LambdaEvent<EventBridgeEvent<Value>>| async {
            log_invocation_start("fxrate-lambda", &event.context);
            handler(event, &snapshots).await
        },
    ))
    .await
}
