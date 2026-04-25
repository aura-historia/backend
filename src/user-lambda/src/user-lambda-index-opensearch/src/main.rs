use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use tracing::debug;
use user::opensearch::repository::UserOpenSearchRepositoryImpl;
use user_lambda_index_opensearch::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let opensearch_client = common::opensearch::client::load_client().await?;
    let repository = UserOpenSearchRepositoryImpl::new(&opensearch_client);

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&repository, event).await
    }))
    .await
}
