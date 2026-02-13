use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use shop::opensearch::repository::ShopOpenSearchRepositoryImpl;
use shop_lambda_opensearch_index::handler;
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let opensearch_client = common::opensearch::client::load_client().await?;
    let repository = ShopOpenSearchRepositoryImpl::new(&opensearch_client);

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&repository, event).await
    }))
    .await
}
