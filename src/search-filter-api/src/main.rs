use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use aws_sdk_dynamodb::Client;
use embedding::{
    EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
    VertexAiEmbeddingConfig, VertexAiEmbeddingGenerator,
};
use google_cloud_auth::credentials::Builder as GoogleCredentialsBuilder;
use lambda_runtime::tracing::{debug, error};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::opensearch::repository::ProductOpenSearchRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product::service::query_service::QueryProductServiceImpl;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use search_filter::service::enhanced_search_match_service::{
    EnhancedSearchMatchService, EnhancedSearchMatchServiceImpl,
};
use search_filter::service::user_search_filter_service::UserSearchFilterServiceImpl;
use search_filter_api::handler;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

const DEFAULT_VERTEX_AI_PROJECT_ID: &str = "project-2c6e1dcc-3fb9-4910-adc";
const DEFAULT_VERTEX_AI_LOCATION: &str = "eu";
const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

struct LocalStackEmbeddingGenerator;

#[async_trait::async_trait]
impl EmbeddingGenerator for LocalStackEmbeddingGenerator {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        Err(EmbeddingError::InvalidInput {
            reason: "LocalStack generator supports queries only",
        })
    }

    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        EmbeddingVector::try_new(vec![0.42; 768])
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let client = Client::new(&aws_config);

    let gemini_api_key = std::env::var("GEMINI_API_KEY")
        .inspect_err(|_| error!("Failed loading GEMINI_API_KEY"))
        .ok();

    let opensearch = common::opensearch::client::load_client()
        .await
        .inspect_err(|e| error!(error = %e, "Failed to initialize OpenSearch client"))?;

    let repository = UserSearchFilterDynamoDbRepositoryImpl::new(&client, &table_name);
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(&client, &table_name);
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(&client, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&client, &table_name);
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(&client, &table_name);
    let product_opensearch_repository = ProductOpenSearchRepositoryImpl::new(&opensearch);

    static NOOP_SES: NoopSesAdapter = NoopSesAdapter;
    static NOOP_S3: NoopS3Adapter = NoopS3Adapter;

    let get_product_service = GetProductServiceImpl::new(&product_dynamodb_repository);
    let query_product_service = QueryProductServiceImpl::new(&product_opensearch_repository);
    let enhanced_match_service: Option<Box<dyn EnhancedSearchMatchService + Sync + Send>> =
        gemini_api_key
            .as_deref()
            .map(|key| Box::new(EnhancedSearchMatchServiceImpl::new(key)) as Box<_>);
    let user_service = UserServiceImpl::new(&user_repository);
    let query_embedding_service: Option<Box<dyn EmbeddingGenerator>> = if std::env::var(
        "LOCALSTACK_HOSTNAME",
    )
    .is_ok()
    {
        Some(Box::new(LocalStackEmbeddingGenerator))
    } else if std::env::var("GOOGLE_APPLICATION_CREDENTIALS").is_ok() {
        let vertex_ai_project_id = std::env::var("VERTEX_AI_PROJECT_ID")
            .unwrap_or_else(|_| DEFAULT_VERTEX_AI_PROJECT_ID.to_string());
        let vertex_ai_location = std::env::var("VERTEX_AI_LOCATION")
            .unwrap_or_else(|_| DEFAULT_VERTEX_AI_LOCATION.to_string());
        GoogleCredentialsBuilder::default()
                .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
                .build_access_token_credentials()
                .map(|credentials| {
                    Box::new(VertexAiEmbeddingGenerator::new(
                        VertexAiEmbeddingConfig::new(vertex_ai_project_id, vertex_ai_location),
                        credentials,
                    )) as Box<dyn EmbeddingGenerator>
                })
                .inspect_err(|embedding_error| {
                    error!(error = %embedding_error, "Failed to initialize embedding generator")
                })
                .ok()
    } else {
        error!(
            "No embedding service configured. Set GOOGLE_APPLICATION_CREDENTIALS or run in LocalStack."
        );
        None
    };
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
    );
    let service = match query_embedding_service.as_deref() {
        Some(query_embedding_service) => UserSearchFilterServiceImpl::with_embedding_service(
            &repository,
            &user_service,
            query_embedding_service,
        ),
        None => UserSearchFilterServiceImpl::new(&repository, &user_service),
    };
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &repository,
    );

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(
                event,
                &service,
                &get_product_service,
                &query_product_service,
                enhanced_match_service.as_deref(),
                &product_personalization_service,
            )
            .await
        },
    ))
    .await
}
