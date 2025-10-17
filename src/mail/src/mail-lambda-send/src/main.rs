use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use mail_core::send_service::SendMailServiceImpl;
use mail_lambda_send::handler;
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

    let s3_bucket_name_templates = std::env::var("S3_BUCKET_NAME_TEMPLATES")?;
    let ses_client = aws_sdk_sesv2::Client::new(&aws_config);
    let s3_client = aws_sdk_s3::Client::new(&aws_config);
    let service = SendMailServiceImpl::new(&ses_client, &s3_client, &s3_bucket_name_templates);

    info!(
        dynamoDbTableName = %s3_bucket_name_templates,
        "Lambda cold start completed, client initialized."
    );

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&service, event).await
    }))
    .await
}
