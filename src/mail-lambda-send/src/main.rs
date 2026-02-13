use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use mail_core::{
    repository::MailDynamoDbRepositoryImpl, s3_adapter::S3AdapterImpl,
    send_service::SendMailServiceImpl, ses_adapter::SesAdapterImpl,
};
use mail_lambda_send::handler;
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let s3_bucket_name_templates = std::env::var("S3_BUCKET_NAME_TEMPLATES")
        .expect("shouldn't fail loading env-var 'S3_BUCKET_NAME_TEMPLATES'");
    let stage_name =
        std::env::var("STAGE_NAME").expect("shouldn't fail loading env-var 'STAGE_NAME'");
    let commit_sha =
        std::env::var("COMMIT_SHA").expect("shouldn't fail loading env-var 'COMMIT_SHA'");
    let ses_client = aws_sdk_sesv2::Client::new(&aws_config);
    let s3_client = aws_sdk_s3::Client::new(&aws_config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let mail_repository = MailDynamoDbRepositoryImpl::new(&dynamodb_client, table_name);
    let s3_adapter = S3AdapterImpl::new(&s3_client);
    let ses_adapter = SesAdapterImpl::new(&ses_client);
    let service = SendMailServiceImpl::new(
        &mail_repository,
        &ses_adapter,
        &s3_adapter,
        &s3_bucket_name_templates,
        &stage_name,
        &commit_sha,
    );

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&service, event).await
    }))
    .await
}
