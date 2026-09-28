use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, log_cold_start, log_invocation_start,
    logging_config_from_env,
};
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use product_listing_ingestion_lambda::{compose_ingestion_use_case, handle_invocation};
use product_listing_service::use_cases::ProcessProductListingIngestionUseCase;
use std::{sync::Arc, time::Instant};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start(
        "product-listing-ingestion-lambda",
        initialization_started_at,
    );
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let credentials = Arc::clone(&credentials);
        let use_cases = Arc::clone(&use_cases);
        async move {
            log_invocation_start("product-listing-ingestion-lambda", &event.context);
            handle_invocation(event, move || async move {
                let credentials = credentials
                    .current()
                    .await
                    .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
                let lease = use_cases
                    .get_or_try_build(credentials.version_id(), || async {
                        let pool = postgres
                            .pool_config(credentials.credentials())
                            .map_err(|_| Error::from("invalid PostgreSQL configuration"))?
                            .connect()
                            .await
                            .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
                        Ok::<Arc<dyn ProcessProductListingIngestionUseCase>, Error>(
                            compose_ingestion_use_case(pool),
                        )
                    })
                    .await?;
                Ok(Arc::clone(lease.value()))
            })
            .await
        }
    }))
    .await
}
