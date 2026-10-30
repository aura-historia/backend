use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, current_postgres_credentials, log_cold_start,
    log_invocation_start, logging_config_from_env,
};
use platform_lambda_sqs::SqsDeadLetterCustody;
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use product_listing_ingestion_lambda::{compose_ingestion_use_case, handle_invocation};
use product_listing_service::use_cases::ProcessProductListingIngestionUseCase;
use std::{sync::Arc, time::Instant};

const COMPONENT: &str = "product-listing-ingestion-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    // The default panic hook prints arbitrary payload text even when FIFO processing catches
    // the panic. Keep diagnostics bounded for both setup and record execution.
    std::panic::set_hook(Box::new(|_| {
        eprintln!("product listing ingestion runtime panicked")
    }));
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let custody = SqsDeadLetterCustody::from_env().await?;
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let credentials = Arc::clone(&credentials);
        let use_cases = Arc::clone(&use_cases);
        let custody = custody.clone();
        async move {
            log_invocation_start(COMPONENT, &event.context);
            handle_invocation(event, &custody, move || async move {
                let credentials =
                    current_postgres_credentials(COMPONENT, credentials.as_ref()).await?;
                let lease = use_cases
                    .get_or_try_build(credentials.version_id(), || async {
                        let pool = postgres
                            .connect(COMPONENT, credentials.credentials())
                            .await?;
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
