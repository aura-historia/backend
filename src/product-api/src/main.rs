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
use product::service::query_service::QueryProductServiceImpl;
use product::service::semantic_service::SemanticSearchServiceImpl;
use product_api::handler;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_pipeline_embed_text::service::{
    MultimodalEmbeddingService, MultimodalEmbeddingServiceImpl,
};
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

const DEFAULT_VERTEX_AI_PROJECT_ID: &str = "aura-historia";
const DEFAULT_VERTEX_AI_LOCATION: &str = "eu";

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
    let user_pool_client_ids = [user_pool_public_client_id.as_str()];
    // Hybrid (BM25 + kNN, OpenSearch-native RRF) search is opt-in via
    // `GOOGLE_APPLICATION_CREDENTIALS`. When unset, the lambda falls back to the
    // existing pure-BM25 query path so the lambda continues to work in
    // environments without an embedding provider. `VERTEX_AI_PROJECT_ID` and
    // `VERTEX_AI_LOCATION` can override the repo defaults used for Vertex.
    let google_application_credentials = std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok();
    let vertex_ai_project_id = std::env::var("VERTEX_AI_PROJECT_ID")
        .unwrap_or_else(|_| DEFAULT_VERTEX_AI_PROJECT_ID.to_string());
    let vertex_ai_location = std::env::var("VERTEX_AI_LOCATION")
        .unwrap_or_else(|_| DEFAULT_VERTEX_AI_LOCATION.to_string());

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
    // The impl now caches `embed_query` results internally via a 4096-entry LRU.
    let query_embedding_service: Option<Box<dyn MultimodalEmbeddingService + Sync + Send>> =
        google_application_credentials.as_ref().map(|_| {
            Box::new(MultimodalEmbeddingServiceImpl::new(
                &vertex_ai_project_id,
                &vertex_ai_location,
            )) as Box<_>
        });
    let query_product_service = QueryProductServiceImpl::new(&product_opensearch_repository);
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
                &query_product_service,
                query_embedding_service.as_deref(),
                &semantic_search_service,
                &product_personalization_service,
                &access_token_verifier_service,
            )
            .await
        },
    ))
    .await
}
