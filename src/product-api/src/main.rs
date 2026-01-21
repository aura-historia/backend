use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use cognito::access_token_verifier_service::AccessTokenVerifierServiceImpl;
use common::price::domain::FixedFxRate;
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::dynamodb::repository::{FxRateDynamoDbRepository, FxRateDynamoDbRepositoryImpl};
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::opensearch::repository::ProductOpenSearchRepositoryImpl;
use product::service::enrichment_service::ProductCommandEnrichmentServiceImpl;
use product::service::get_service::GetProductServiceImpl;
use product::service::personalization_service::ProductPersonalizationServiceImpl;
use product::service::query_service::QueryProductServiceImpl;
use product::service::semantic_service::SemanticSearchServiceImpl;
use product::service::upsert_service::UpsertProductsServiceImpl;
use product::watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use product::watchlist::service::product_watchlist_service::ProductWatchListServiceImpl;
use product_api::handler;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use tracing::{error, warn};
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;

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
    let user_pool_id = std::env::var("USER_POOL_ID")?;
    let user_pool_public_client_id = std::env::var("USER_POOL_PUBLIC_CLIENT_ID")?;
    let user_pool_admin_client_id = std::env::var("USER_POOL_ADMIN_CLIENT_ID")?;
    let user_pool_client_ids = [
        user_pool_public_client_id.as_str(),
        user_pool_admin_client_id.as_str(),
    ];

    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);
    let opensearch = common::opensearch::client::load_client()
        .await
        .expect("shouldn't fail loading OpenSearch-Client");

    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let shop_dynamodb_repository = ShopDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let product_opensearch_repository = ProductOpenSearchRepositoryImpl::new(&opensearch);
    let fxrate_repository = FxRateDynamoDbRepositoryImpl::new(&dynamodb, &table_name);

    let get_product_service = GetProductServiceImpl::new(&product_dynamodb_repository);
    let query_product_service = QueryProductServiceImpl::new(&product_opensearch_repository);
    let semantic_search_service = SemanticSearchServiceImpl::new(
        &product_dynamodb_repository,
        &product_opensearch_repository,
    );
    let product_watchlist_service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_dynamodb_repository,
        &get_product_service,
    );
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository);
    let fx_rate = fxrate_repository
        .get_fx_rates_record()
        .await
        .unwrap_or_else(|err| {
            error!(error = ?err, "Failed loading FxRate from DynamoDB. Defaulting to FixedFxRate.");
            Some(FxRatesRecord::from(FixedFxRate()))
        })
        .unwrap_or_else(|| {
            warn!("There was no FxRatesRecord in DynamoDB. Defaulting to FixedFxRate.");
            FxRatesRecord::from(FixedFxRate())
        });
    let upsert_service = UpsertProductsServiceImpl::new(&product_dynamodb_repository, &fx_rate);
    let enrich_service =
        ProductCommandEnrichmentServiceImpl::new(&shop_dynamodb_repository, &fx_rate);

    let access_token_verifier_service = AccessTokenVerifierServiceImpl::new(
        "eu-central-1",
        &user_pool_id,
        user_pool_client_ids.as_slice(),
    )
    .expect("shouldn't fail creating 'AccessTokenVerifierServiceImpl'");

    info!("Lambda cold start completed, client initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(
                event,
                &get_product_service,
                &query_product_service,
                &semantic_search_service,
                &product_watchlist_service,
                &product_personalization_service,
                &upsert_service,
                &enrich_service,
                &access_token_verifier_service,
            )
            .await
        },
    ))
    .await
}
