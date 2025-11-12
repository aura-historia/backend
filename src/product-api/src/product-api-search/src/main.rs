use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use cognito::access_token_verifier_service::AccessTokenVerifierServiceImpl;
use item_api_search::handler;
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use opensearch::http::Url;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use product::opensearch::repository::ProductOpenSearchRepositoryImpl;
use product::service::personalization_service::ItemPersonalizationServiceImpl;
use product::service::query_service::QueryItemServiceImpl;
use product::watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use std::env;

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

    let domain_endpoint = env::var("OPENSEARCH_DOMAIN_ENDPOINT_URL")?;
    let domain_endpoint_url = Url::parse(&domain_endpoint)?;
    let transport = TransportBuilder::new(SingleNodeConnectionPool::new(domain_endpoint_url))
        .auth(aws_config.clone().try_into()?)
        .service_name("es")
        .build()?;
    let opensearch_client = opensearch::OpenSearch::new(transport);
    let item_opensearch_repository = ProductOpenSearchRepositoryImpl::new(&opensearch_client);
    let query_item_service = QueryItemServiceImpl::new(&item_opensearch_repository);

    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let table_name = std::env::var("DYNAMODB_TABLE_NAME")?;
    let watchlist_repository =
        WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let item_personalization_service = ItemPersonalizationServiceImpl::new(&watchlist_repository);

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

    info!(
        domainEndpointUrl = %domain_endpoint,
        "Lambda cold start completed, client initialized."
    );

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(
                event,
                &query_item_service,
                &access_token_verifier_service,
                &item_personalization_service,
            )
            .await
        },
    ))
    .await
}
