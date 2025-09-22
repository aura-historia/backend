use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use opensearch::http::Url;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use shop_api_search::handler;
use shop_opensearch::repository::ShopOpenSearchRepositoryImpl;
use shop_service::query_service::QueryShopServiceImpl;
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
        .auth(aws_config.try_into()?)
        .service_name("es")
        .build()?;
    let client = opensearch::OpenSearch::new(transport);
    let repository = ShopOpenSearchRepositoryImpl::new(&client);
    let service = QueryShopServiceImpl::new(&repository);

    info!(
        domainEndpointUrl = %domain_endpoint,
        "Lambda cold start completed, client initialized."
    );

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async { handler(event, &service).await },
    ))
    .await
}
