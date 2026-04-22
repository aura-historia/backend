use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use cognito::load_access_token_verifier_service;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::opensearch::repository::ProductOpenSearchRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product::service::query_embedding_service::{
    CachedQueryEmbeddingService, GeminiQueryEmbeddingService,
};
use product::service::query_service::QueryProductServiceImpl;
use product::service::semantic_service::SemanticSearchServiceImpl;
use product_api::handler;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use std::time::Duration;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let user_pool_id =
        std::env::var("USER_POOL_ID").expect("shouldn't fail loading env-var 'USER_POOL_ID'");
    let user_pool_public_client_id = std::env::var("USER_POOL_PUBLIC_CLIENT_ID")
        .expect("shouldn't fail loading env-var 'USER_POOL_PUBLIC_CLIENT_ID'");
    let user_pool_admin_client_id = std::env::var("USER_POOL_ADMIN_CLIENT_ID")
        .expect("shouldn't fail loading env-var 'USER_POOL_ADMIN_CLIENT_ID'");
    let user_pool_client_ids = [
        user_pool_public_client_id.as_str(),
        user_pool_admin_client_id.as_str(),
    ];
    // Hybrid search is opt-in via the GEMINI_API_KEY env-var. When unset, the lambda
    // falls back to the existing pure-BM25 query path so the lambda continues to work
    // in environments without an embedding provider configured.
    let gemini_api_key = std::env::var("GEMINI_API_KEY").ok();

    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);
    let opensearch = common::opensearch::client::load_client()
        .await
        .expect("shouldn't fail loading OpenSearch-Client");

    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let product_opensearch_repository = ProductOpenSearchRepositoryImpl::new(&opensearch);
    let search_filter_repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(&dynamodb, &table_name);

    static NOOP_SES: NoopSesAdapter = NoopSesAdapter;
    static NOOP_S3: NoopS3Adapter = NoopS3Adapter;

    let get_product_service = GetProductServiceImpl::new(&product_dynamodb_repository);
    // 256 entries × 768 f32 ≈ 770 KB — bounded so the warm Lambda stays lightweight.
    let query_embedding_service = gemini_api_key.as_deref().map(|key| {
        CachedQueryEmbeddingService::new(
            GeminiQueryEmbeddingService::new(key),
            Duration::from_secs(300),
            256,
        )
    });
    // Use a boxed trait object so the same handler call site works whether or not we have
    // an embedding service configured (the two `QueryProductServiceImpl` instantiations
    // would otherwise have different concrete types).
    let query_product_service: Box<dyn product::service::query_service::QueryProductService + Sync + Send> =
        match query_embedding_service.as_ref() {
            Some(embedding_service) => Box::new(QueryProductServiceImpl::with_hybrid(
                &product_opensearch_repository,
                embedding_service,
            )),
            None => Box::new(QueryProductServiceImpl::new(&product_opensearch_repository)),
        };
    let semantic_search_service = SemanticSearchServiceImpl::new(
        &product_dynamodb_repository,
        &product_opensearch_repository,
    );
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
    );
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );

    let access_token_verifier_service =
        load_access_token_verifier_service(&user_pool_id, &user_pool_client_ids);

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(
                event,
                &get_product_service,
                query_product_service.as_ref(),
                &semantic_search_service,
                &product_personalization_service,
                &access_token_verifier_service,
            )
            .await
        },
    ))
    .await
}
