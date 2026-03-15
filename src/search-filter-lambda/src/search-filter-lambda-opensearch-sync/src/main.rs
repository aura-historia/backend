use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use search_filter::opensearch::repository::UserSearchFilterOpenSearchRepositoryImpl;
use search_filter_lambda_opensearch_sync::handler;
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let opensearch_client = common::opensearch::client::load_client().await?;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(&opensearch_client);

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&repository, event).await
    }))
    .await
}
