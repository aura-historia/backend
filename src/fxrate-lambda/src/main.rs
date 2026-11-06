use aws_lambda_events::eventbridge::EventBridgeEvent;
use fxrate_fxratesapi::FxRatesApiQuoteProvider;
use fxrate_lambda::handler;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use fxrate_service::CaptureFxRateSnapshotHandler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LazyComposition, RuntimePostgresConfig, log_cold_start, log_invocation_start,
    logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use serde_json::Value;
use std::{sync::Arc, time::Instant};

const COMPONENT: &str = "fxrate-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = RuntimePostgresConfig::from_env()?;
    let token = std::env::var("FXRATES_API_TOKEN")
        .map_err(|_| Error::from("missing required environment variable FXRATES_API_TOKEN"))?;
    let snapshots = Arc::new(LazyComposition::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(
        move |event: LambdaEvent<EventBridgeEvent<Value>>| {
            let postgres = postgres.clone();
            let token = token.clone();
            let snapshots = Arc::clone(&snapshots);
            async move {
                log_invocation_start(COMPONENT, &event.context);
                let snapshots = snapshots
                    .get_or_try_build(|| async {
                        let pool = postgres.connect(COMPONENT).await?;
                        Ok::<_, Error>(Arc::new(CaptureFxRateSnapshotHandler::new(
                            FxRatesApiQuoteProvider::new(reqwest::Client::new(), token),
                            SqlxUnitOfWork::new(pool),
                            SqlxFxRateSnapshotRepositoryFactory::new(),
                        )))
                    })
                    .await?;

                handler(event, snapshots.as_ref()).await
            }
        },
    ))
    .await
}
