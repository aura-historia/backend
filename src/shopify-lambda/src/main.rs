use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use listing_source_postgres::SqlxListingSourceReaders;
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, log_cold_start, log_invocation_start,
    logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingRawCaptureWriterFactory,
};
use product_listing_service::use_cases::CaptureProductListingRawObservationHandler;
use shopify_lambda::{ShopifyProductListingProcessor, handler};
use std::{sync::Arc, time::Instant};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let processors = Arc::new(VersionedCompositionCache::new());

    log_cold_start("shopify-lambda", initialization_started_at);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let credentials = Arc::clone(&credentials);
        let processors = Arc::clone(&processors);
        async move {
            log_invocation_start("shopify-lambda", &event.context);
            let credentials = credentials
                .current()
                .await
                .map_err(|_| Error::from("PostgreSQL credential refresh unavailable"))?;
            let processor = processors
                .get_or_try_build(credentials.version_id(), || async {
                    let pool = postgres
                        .pool_config(credentials.credentials())
                        .map_err(|_| Error::from("invalid PostgreSQL configuration"))?
                        .connect()
                        .await
                        .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
                    Ok::<_, Error>(ShopifyProductListingProcessor::new(
                        SqlxListingSourceReaders::new(pool.clone()),
                        CaptureProductListingRawObservationHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxProductListingRawCaptureWriterFactory::new(),
                            SqlxPartnerProductListingAuthorizerFactory::new(),
                        ),
                    ))
                })
                .await?;

            handler(event, processor.value()).await
        }
    }))
    .await
}
