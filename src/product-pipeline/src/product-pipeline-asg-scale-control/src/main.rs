use aws_config::BehaviorVersion;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product_pipeline_asg_scale_control::{SqsAsgComponent, handler};
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

    let autoscaling = aws_sdk_autoscaling::Client::new(&aws_config);
    let sqs = aws_sdk_sqs::Client::new(&aws_config);
    let cloudwatch = aws_sdk_cloudwatch::Client::new(&aws_config);
    let components = vec![
        SqsAsgComponent {
            sqs_url: std::env::var("PRODUCT_PIPELINE_INIT_SQS_URL")?,
            queue_name: std::env::var("PRODUCT_PIPELINE_INIT_SQS_NAME")?,
            asg_name: std::env::var("PRODUCT_PIPELINE_INIT_ASG_NAME")?,
        },
        SqsAsgComponent {
            sqs_url: std::env::var("PRODUCT_PIPELINE_TRANSLATE_SQS_URL")?,
            queue_name: std::env::var("PRODUCT_PIPELINE_TRANSLATE_SQS_NAME")?,
            asg_name: std::env::var("PRODUCT_PIPELINE_TRANSLATE_ASG_NAME")?,
        },
        SqsAsgComponent {
            sqs_url: std::env::var("PRODUCT_PIPELINE_EMBED_TEXT_SQS_URL")?,
            queue_name: std::env::var("PRODUCT_PIPELINE_EMBED_TEXT_SQS_NAME")?,
            asg_name: std::env::var("PRODUCT_PIPELINE_EMBED_TEXT_ASG_NAME")?,
        },
    ];

    info!("Lambda cold start completed, client initialized.");

    run(service_fn(|event: LambdaEvent<serde_json::Value>| async {
        handler(&autoscaling, &sqs, &cloudwatch, &components, event).await
    }))
    .await
}
