use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use shop::opensearch::repository::ShopOpenSearchRepositoryImpl;
use shop::service::query_service::QueryShopServiceImpl;
use shop_api_search::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    let opensearch_client = common::opensearch::client::load_client().await?;
    let repository = ShopOpenSearchRepositoryImpl::new(&opensearch_client);
    let service = QueryShopServiceImpl::new(&repository);

    info!("Lambda cold start completed, client initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async { handler(event, &service).await },
    ))
    .await
}
