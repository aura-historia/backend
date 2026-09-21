use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use listing_source_postgres::SqlxListingSourceReaders;
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, log_cold_start, log_invocation_start, logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingRawCaptureWriterFactory,
};
use product_listing_service::use_cases::CaptureProductListingRawObservationHandler;
use shopify_lambda::{ShopifyProductListingProcessor, handler};
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
    let processor = ShopifyProductListingProcessor::new(
        SqlxListingSourceReaders::new(pool.clone()),
        CaptureProductListingRawObservationHandler::new(
            SqlxUnitOfWork::new(pool),
            SqlxProductListingRawCaptureWriterFactory::new(),
            SqlxPartnerProductListingAuthorizerFactory::new(),
        ),
    );

    log_cold_start("shopify-lambda", initialization_started_at);
    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        log_invocation_start("shopify-lambda", &event.context);
        handler(event, &processor).await
    }))
    .await
}
