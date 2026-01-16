use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use cognito::access_token_verifier_service::AccessTokenVerifierServiceImpl;
use common::opensearch::client::load_client;
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::opensearch::repository::ProductOpenSearchRepositoryImpl;
use product::service::personalization_service::ProductPersonalizationServiceImpl;
use product::service::semantic_service::SemanticSearchServiceImpl;
use product::watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use product_api_get_product_similar::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")?;
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let opensearch_client = load_client().await?;
    let product_opensearch_repository = ProductOpenSearchRepositoryImpl::new(&opensearch_client);
    let product_dynamodb_repository =
        ProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let semantic_search_service = SemanticSearchServiceImpl::new(
        &product_dynamodb_repository,
        &product_opensearch_repository,
    );

    let watchlist_repository =
        WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository);

    let user_pool_id = std::env::var("USER_POOL_ID")?;
    let user_pool_public_client_id = std::env::var("USER_POOL_PUBLIC_CLIENT_ID")?;
    let user_pool_admin_client_id = std::env::var("USER_POOL_ADMIN_CLIENT_ID")?;
    let client_ids = [
        user_pool_public_client_id.as_str(),
        user_pool_admin_client_id.as_str(),
    ];
    let access_token_verifier_service =
        AccessTokenVerifierServiceImpl::new("eu-central-1", &user_pool_id, client_ids.as_slice())
            .expect("shouldn't fail creating 'AccessTokenVerifierServiceImpl'");

    info!("Lambda cold start completed, client initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(
                event,
                &semantic_search_service,
                &access_token_verifier_service,
                &product_personalization_service,
            )
            .await
        },
    ))
    .await
}
