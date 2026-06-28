use aws_config::BehaviorVersion;
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
use search_filter::{
    opensearch::repository::UserSearchFilterOpenSearchRepositoryImpl,
    service::{
        enhanced_search_match_service::EnhancedSearchMatchServiceImpl,
        user_search_filter_service::UserSearchFilterServiceImpl,
    },
};
use search_filter_periodic_match::{
    DEFAULT_LLM_CONCURRENCY, PeriodicMatcherService, PeriodicMatcherServiceImpl,
};
use tracing::{debug, info};
use user::{
    dynamodb::repository::UserDynamoDbRepositoryImpl, service::user_service::UserServiceImpl,
};

const DEFAULT_GEMINI_MODEL: &str = "gemini-3.1-flash-lite";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let ses_client = aws_sdk_sesv2::Client::new(&aws_config);
    let s3_client = aws_sdk_s3::Client::new(&aws_config);

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")?;
    let s3_bucket_name_templates = std::env::var("S3_BUCKET_NAME_TEMPLATES")?;
    let stage_name = std::env::var("STAGE_NAME")?;
    let commit_sha = std::env::var("COMMIT_SHA")?;
    let gemini_api_key = std::env::var("GEMINI_API_KEY")?;
    let gemini_model =
        std::env::var("GEMINI_MODEL").unwrap_or_else(|_| DEFAULT_GEMINI_MODEL.to_string());
    let llm_concurrency = std::env::var("PERIODIC_MATCH_LLM_CONCURRENCY")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_LLM_CONCURRENCY);

    let opensearch_client = common::opensearch::client::load_client().await?;
    let product_opensearch_repo = ProductOpenSearchRepositoryImpl::new(&opensearch_client);
    let query_product_service = QueryProductServiceImpl::new(&product_opensearch_repo);
    let search_filter_opensearch_repo =
        UserSearchFilterOpenSearchRepositoryImpl::new(&opensearch_client);
    let search_filter_dynamodb_repo =
        search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl::new(
            &dynamodb_client,
            &table_name,
        );
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let user_service = UserServiceImpl::new(&user_repository);
    let user_search_filter_service = UserSearchFilterServiceImpl::with_opensearch(
        &search_filter_dynamodb_repo,
        &user_service,
        &search_filter_opensearch_repo,
    );
    let enhanced_search_match_service =
        EnhancedSearchMatchServiceImpl::new_with_model(&gemini_api_key, &gemini_model, true);
    let notification_repository =
        NotificationDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
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
        &enhanced_search_match_service,
        &notification_service,
        &user_service,
        llm_concurrency,
    );

    debug!(
        geminiModel = %gemini_model,
        geminiFlex = true,
        llmConcurrency = llm_concurrency,
        "Periodic search-filter matcher initialized."
    );

    info!("Started periodic hybrid-search search-filter matching.");
    let result = matcher_service.match_active_filters().await?;
    info!(
        filtersProcessed = result.filters_processed,
        matchesCreated = result.matches_created,
        notificationsCreated = result.notifications_created,
        filtersFailed = result.filters_failed,
        "Finished periodic hybrid-search search-filter matching."
    );

    if result.filters_failed > 0 {
        return Err(format!(
            "periodic hybrid-search search-filter matching failed for {} filter(s)",
            result.filters_failed
        )
        .into());
    }

    Ok(())
}
