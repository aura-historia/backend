use aws_config::BehaviorVersion;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product_enrichment_asg_scale_up::handler;
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

    let aws_config = aws_config::defaults(BehaviorVersion::v2025_08_07())
        .load()
        .await;

    let autoscaling_client = aws_sdk_autoscaling::Client::new(&aws_config);
    let asg_name = std::env::var("PRODUCT_ENRICHMENT_ASG_NAME")?;

    let opensearch_client = common::opensearch::client::load_client().await?;

    info!(
        asgName = asg_name,
        "Lambda cold start completed, client initialized."
    );

    run(service_fn(|event: LambdaEvent<serde_json::Value>| async {
        handler(&autoscaling_client, &opensearch_client, &asg_name, event).await
    }))
    .await
}
