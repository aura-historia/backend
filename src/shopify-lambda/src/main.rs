use aws_lambda_events::sqs::SqsEvent;
use common::postgres::SqlxUnitOfWork;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product_postgres::{
    SqlxPartnerProductAuthorizerFactory, SqlxProductEventStoreFactory, SqlxProductRepositoryFactory,
};
use product_service::use_cases::{IngestShopifyProductHandler, UpsertProductHandler};
use shop_postgres::SqlxShopDetailsReaderFactory;
use shop_service::use_cases::GetShopHandler;
use shopify_lambda::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let pool = common::postgres::connect_from_env().await?;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let ingestion = IngestShopifyProductHandler::new(
        GetShopHandler::new(unit_of_work.clone(), SqlxShopDetailsReaderFactory::new()),
        UpsertProductHandler::new_with_fx_rates(
            unit_of_work,
            SqlxProductRepositoryFactory::new(),
            SqlxProductEventStoreFactory::new(),
            SqlxPartnerProductAuthorizerFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
        ),
    );

    debug!("Shopify Lambda initialized");
    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(event, &ingestion).await
    }))
    .await
}
