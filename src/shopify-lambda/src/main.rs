use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use listing_source_postgres::SqlxListingSourceReaders;
use platform_lambda_bootstrap::{
    LazyComposition, RuntimePostgresConfig, log_cold_start, log_invocation_start,
    logging_config_from_env,
};
use platform_observability::init;
use product_listing_ingestion_sqs::{
    ScopedSqsProductListingIngestionPublisher, SqsProductListingIngestionPublisher,
    with_publication_deadline,
};
use product_listing_service::use_cases::SubmitInternalProductListingIngestionHandler;
use product_listing_service::use_cases::commands::process_shopify_product_listing::ProcessShopifyProductListingHandler;
use product_listing_shopify::ShopifyProductPayloadDecoder;
use shopify_lambda::{handler, publication_deadline};
use std::{sync::Arc, time::Instant};

const COMPONENT: &str = "shopify-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = RuntimePostgresConfig::from_env()?;
    let queue_url = std::env::var("PRODUCT_LISTING_INGESTION_QUEUE_URL")
        .map_err(|_| Error::from("product listing ingestion queue URL unavailable"))?;
    let aws_config = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let sqs_client = aws_sdk_sqs::Client::new(&aws_config);
    let processors = Arc::new(LazyComposition::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let processors = Arc::clone(&processors);
        let sqs_client = sqs_client.clone();
        let queue_url = queue_url.clone();
        async move {
            log_invocation_start(COMPONENT, &event.context);
            let deadline = publication_deadline(&event.context);
            with_publication_deadline(deadline, async move {
                let processor = processors
                    .get_or_try_build(|| async {
                        let pool = postgres.connect(COMPONENT).await?;
                        Ok::<_, Error>(Arc::new(ProcessShopifyProductListingHandler::new(
                            SqlxListingSourceReaders::new(pool),
                            SubmitInternalProductListingIngestionHandler::new(
                                ScopedSqsProductListingIngestionPublisher::new(
                                    SqsProductListingIngestionPublisher::new(sqs_client, queue_url),
                                ),
                            ),
                            ShopifyProductPayloadDecoder,
                        )))
                    })
                    .await?;

                handler(event, processor.as_ref()).await
            })
            .await
        }
    }))
    .await
}
