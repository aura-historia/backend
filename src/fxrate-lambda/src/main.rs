use aws_lambda_events::eventbridge::EventBridgeEvent;
use fxrate_fxratesapi::FxRatesApiQuoteProvider;
use fxrate_lambda::handler;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use fxrate_service::CaptureFxRateSnapshotHandler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, current_postgres_credentials, log_cold_start,
    log_invocation_start, logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use serde_json::Value;
use std::{sync::Arc, time::Instant};

const COMPONENT: &str = "fxrate-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let token = std::env::var("FXRATES_API_TOKEN")
        .map_err(|_| Error::from("missing required environment variable FXRATES_API_TOKEN"))?;
    let snapshots = Arc::new(VersionedCompositionCache::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(
        move |event: LambdaEvent<EventBridgeEvent<Value>>| {
            let postgres = postgres.clone();
            let credentials = Arc::clone(&credentials);
            let token = token.clone();
            let snapshots = Arc::clone(&snapshots);
            async move {
                log_invocation_start(COMPONENT, &event.context);
                let credentials =
                    current_postgres_credentials(COMPONENT, credentials.as_ref()).await?;
                let snapshots = snapshots
                    .get_or_try_build(credentials.version_id(), || async {
                        let pool = postgres
                            .connect(COMPONENT, credentials.credentials())
                            .await?;
                        Ok::<_, Error>(CaptureFxRateSnapshotHandler::new(
                            FxRatesApiQuoteProvider::new(reqwest::Client::new(), token),
                            SqlxUnitOfWork::new(pool),
                            SqlxFxRateSnapshotRepositoryFactory::new(),
                        ))
                    })
                    .await?;

                handler(event, snapshots.value()).await
            }
        },
    ))
    .await
}
