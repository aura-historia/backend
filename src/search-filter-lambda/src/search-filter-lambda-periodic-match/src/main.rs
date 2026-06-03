use aws_config::BehaviorVersion;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification::{
    dynamodb::repository::NotificationDynamoDbRepositoryImpl,
    service::{
        notification_service::NotificationServiceImpl, s3_adapter::S3AdapterImpl,
        ses_adapter::SesAdapterImpl,
    },
};
use product::{
    opensearch::repository::ProductOpenSearchRepositoryImpl,
    service::query_service::QueryProductServiceImpl,
};
use product_pipeline_embed_text::service::MultimodalEmbeddingServiceImpl;
use search_filter::{
    opensearch::repository::UserSearchFilterOpenSearchRepositoryImpl,
    service::{
        enhanced_search_match_service::EnhancedSearchMatchServiceImpl,
        user_search_filter_service::UserSearchFilterServiceImpl,
    },
};
use search_filter_lambda_periodic_match::{handler, service::PeriodicMatcherServiceImpl};
use tracing::debug;
use user::{
    dynamodb::repository::UserDynamoDbRepositoryImpl, service::user_service::UserServiceImpl,
};

const DEFAULT_VERTEX_AI_PROJECT_ID: &str = "aura-historia";
const DEFAULT_VERTEX_AI_LOCATION: &str = "eu";

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let client = aws_sdk_dynamodb::Client::new(&config);
    let ses_client = aws_sdk_sesv2::Client::new(&config);
    let s3_client = aws_sdk_s3::Client::new(&config);

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")?;
    let s3_bucket_name_templates = std::env::var("S3_BUCKET_NAME_TEMPLATES")?;
    let stage_name = std::env::var("STAGE_NAME")?;
    let commit_sha = std::env::var("COMMIT_SHA")?;
    let _google_application_credentials = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")?;
    let gemini_api_key = std::env::var("GEMINI_API_KEY")?;
    let vertex_ai_project_id = std::env::var("VERTEX_AI_PROJECT_ID")
        .unwrap_or_else(|_| DEFAULT_VERTEX_AI_PROJECT_ID.to_string());
    let vertex_ai_location = std::env::var("VERTEX_AI_LOCATION")
        .unwrap_or_else(|_| DEFAULT_VERTEX_AI_LOCATION.to_string());

    let opensearch_client = common::opensearch::client::load_client().await?;
    let product_opensearch_repo = ProductOpenSearchRepositoryImpl::new(&opensearch_client);
    let query_product_service = QueryProductServiceImpl::new(&product_opensearch_repo);
    let search_filter_opensearch_repo =
        UserSearchFilterOpenSearchRepositoryImpl::new(&opensearch_client);
    let search_filter_dynamodb_repo =
        search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl::new(
            &client,
            &table_name,
        );
    let user_repository = UserDynamoDbRepositoryImpl::new(&client, &table_name);
    let user_service = UserServiceImpl::new(&user_repository);
    let user_search_filter_service = UserSearchFilterServiceImpl::with_opensearch(
        &search_filter_dynamodb_repo,
        &user_service,
        &search_filter_opensearch_repo,
    );
    let embedding_service =
        MultimodalEmbeddingServiceImpl::new(&vertex_ai_project_id, &vertex_ai_location);
    let enhanced_search_match_service = EnhancedSearchMatchServiceImpl::new(&gemini_api_key);
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(&client, &table_name);
    let ses_adapter = SesAdapterImpl::new(&ses_client);
    let s3_adapter = S3AdapterImpl::new(&s3_client);
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &ses_adapter,
        &s3_adapter,
        &s3_bucket_name_templates,
        &stage_name,
        &commit_sha,
    );
    let matcher_service = PeriodicMatcherServiceImpl::new(
        &user_search_filter_service,
        &query_product_service,
        &embedding_service,
        &enhanced_search_match_service,
        &notification_service,
        &user_service,
    );

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<serde_json::Value>| async {
        handler(&matcher_service, event).await
    }))
    .await
}
