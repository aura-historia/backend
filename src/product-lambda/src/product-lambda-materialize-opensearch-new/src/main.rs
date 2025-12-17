use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::opensearch::repository::ProductOpenSearchRepositoryImpl;
use product_lambda_materialize_opensearch_new::handler;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    let opensearch_client = common::opensearch::client::load_client().await?;
    let repository = ProductOpenSearchRepositoryImpl::new(&opensearch_client);

    info!("Lambda cold start completed, OpenSearch-Client initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&repository, event).await
    }))
    .await
}
