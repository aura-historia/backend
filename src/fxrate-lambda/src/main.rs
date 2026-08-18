use aws_lambda_events::eventbridge::EventBridgeEvent;
use common::postgres::SqlxUnitOfWork;
use fxrate_fxratesapi::FxRatesApiQuoteProvider;
use fxrate_lambda::handler;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use fxrate_service::CaptureFxRateSnapshotHandler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use serde_json::Value;
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let pool = common::postgres::connect_from_env().await?;
    let token = std::env::var("FXRATES_API_TOKEN")
        .map_err(|_| Error::from("missing required environment variable FXRATES_API_TOKEN"))?;
    let snapshots = CaptureFxRateSnapshotHandler::new(
        FxRatesApiQuoteProvider::new(reqwest::Client::new(), token),
        SqlxUnitOfWork::new(pool),
        SqlxFxRateSnapshotRepositoryFactory::new(),
    );

    debug!("FX rate Lambda initialized");
    run(service_fn(
        |event: LambdaEvent<EventBridgeEvent<Value>>| async { handler(event, &snapshots).await },
    ))
    .await
}
