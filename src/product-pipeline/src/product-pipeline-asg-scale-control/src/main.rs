use aws_config::BehaviorVersion;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product_pipeline_asg_scale_control::{SqsAsgComponent, handler};
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let autoscaling = aws_sdk_autoscaling::Client::new(&aws_config);
    let sqs = aws_sdk_sqs::Client::new(&aws_config);
    let cloudwatch = aws_sdk_cloudwatch::Client::new(&aws_config);
    let components = vec![
        SqsAsgComponent {
            sqs_url: std::env::var("PRODUCT_PIPELINE_TRANSLATE_SQS_URL")
                .expect("shouldn't fail loading env-var 'PRODUCT_PIPELINE_TRANSLATE_SQS_URL'"),
            queue_name: std::env::var("PRODUCT_PIPELINE_TRANSLATE_SQS_NAME")
                .expect("shouldn't fail loading env-var 'PRODUCT_PIPELINE_TRANSLATE_SQS_NAME'"),
            asg_name: std::env::var("PRODUCT_PIPELINE_TRANSLATE_ASG_NAME")
                .expect("shouldn't fail loading env-var 'PRODUCT_PIPELINE_TRANSLATE_ASG_NAME'"),
        },
        SqsAsgComponent {
            sqs_url: std::env::var("PRODUCT_PIPELINE_EMBED_TEXT_SQS_URL")
                .expect("shouldn't fail loading env-var 'PRODUCT_PIPELINE_EMBED_TEXT_SQS_URL'"),
            queue_name: std::env::var("PRODUCT_PIPELINE_EMBED_TEXT_SQS_NAME")
                .expect("shouldn't fail loading env-var 'PRODUCT_PIPELINE_EMBED_TEXT_SQS_NAME'"),
            asg_name: std::env::var("PRODUCT_PIPELINE_EMBED_TEXT_ASG_NAME")
                .expect("shouldn't fail loading env-var 'PRODUCT_PIPELINE_EMBED_TEXT_ASG_NAME'"),
        },
        SqsAsgComponent {
            sqs_url: std::env::var("PRODUCT_PIPELINE_EXTRACT_ATTRIBUTE_SQS_URL").expect(
                "shouldn't fail loading env-var 'PRODUCT_PIPELINE_EXTRACT_ATTRIBUTE_SQS_URL'",
            ),
            queue_name: std::env::var("PRODUCT_PIPELINE_EXTRACT_ATTRIBUTE_SQS_NAME").expect(
                "shouldn't fail loading env-var 'PRODUCT_PIPELINE_EXTRACT_ATTRIBUTE_SQS_NAME'",
            ),
            asg_name: std::env::var("PRODUCT_PIPELINE_EXTRACT_ATTRIBUTE_ASG_NAME").expect(
                "shouldn't fail loading env-var 'PRODUCT_PIPELINE_EXTRACT_ATTRIBUTE_ASG_NAME'",
            ),
        },
        SqsAsgComponent {
            sqs_url: std::env::var("PRODUCT_PIPELINE_CLASSIFY_CATEGORY_SQS_URL").expect(
                "shouldn't fail loading env-var 'PRODUCT_PIPELINE_CLASSIFY_CATEGORY_SQS_URL'",
            ),
            queue_name: std::env::var("PRODUCT_PIPELINE_CLASSIFY_CATEGORY_SQS_NAME").expect(
                "shouldn't fail loading env-var 'PRODUCT_PIPELINE_CLASSIFY_CATEGORY_SQS_NAME'",
            ),
            asg_name: std::env::var("PRODUCT_PIPELINE_CLASSIFY_CATEGORY_ASG_NAME").expect(
                "shouldn't fail loading env-var 'PRODUCT_PIPELINE_CLASSIFY_CATEGORY_ASG_NAME'",
            ),
        },
    ];

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<serde_json::Value>| async {
        handler(&autoscaling, &sqs, &cloudwatch, &components, event).await
    }))
    .await
}
