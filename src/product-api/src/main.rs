use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use cognito::access_token_verifier_service::{
    AccessTokenVerifierService, AccessTokenVerifierServiceImpl,
};
use cognito::localstack_access_token_verifier_service::LocalStackAccessTokenVerifierServiceImpl;
use common::price::domain::FixedFxRate;
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::dynamodb::repository::{FxRateDynamoDbRepository, FxRateDynamoDbRepositoryImpl};
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::opensearch::repository::ProductOpenSearchRepositoryImpl;
use product::service::enrichment_service::ProductCommandEnrichmentServiceImpl;
use product::service::get_service::GetProductServiceImpl;
use product::service::query_service::QueryProductServiceImpl;
use product::service::semantic_service::SemanticSearchServiceImpl;
use product::service::upsert_service::UpsertProductsServiceImpl;
use product_api::handler;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use tracing::{error, warn};
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

    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);
    let opensearch = common::opensearch::client::load_client()
        .await
        .expect("shouldn't fail loading OpenSearch-Client");

    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let shop_dynamodb_repository = ShopDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let product_opensearch_repository = ProductOpenSearchRepositoryImpl::new(&opensearch);
    let fxrate_repository = FxRateDynamoDbRepositoryImpl::new(&dynamodb, &table_name);

    static NOOP_SES: NoopSesAdapter = NoopSesAdapter;
    static NOOP_S3: NoopS3Adapter = NoopS3Adapter;

    let get_product_service = GetProductServiceImpl::new(&product_dynamodb_repository);
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
        "noreply@example.com"
            .parse()
            .expect("shouldn't fail parsing placeholder sender email"),
    );
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );
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

    let access_token_verifier_service: Box<dyn AccessTokenVerifierService + Sync + Send> =
        match std::env::var("LOCALSTACK_HOSTNAME") {
            Ok(_) => {
                let mapped_port =
                    std::env::var("LOCALSTACK_MAPPED_PORT").unwrap_or_else(|_| "4566".to_owned());
                // `host.docker.internal` resolves inside the Lambda container thanks to
                // `--add-host=host.docker.internal:host-gateway` in LAMBDA_DOCKER_FLAGS.
                let cognito_idp_endpoint = format!("http://host.docker.internal:{mapped_port}");
                Box::new(
                    LocalStackAccessTokenVerifierServiceImpl::new(
                        &cognito_idp_endpoint,
                        "eu-central-1",
                        &user_pool_id,
                        user_pool_client_ids.as_slice(),
                    )
                    .expect("shouldn't fail creating 'LocalStackAccessTokenVerifierServiceImpl'"),
                )
            }
            Err(_) => Box::new(
                AccessTokenVerifierServiceImpl::new(
                    "eu-central-1",
                    &user_pool_id,
                    user_pool_client_ids.as_slice(),
                )
                .expect("shouldn't fail creating 'AccessTokenVerifierServiceImpl'"),
            ),
        };

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(
                event,
                &get_product_service,
                &query_product_service,
                &semantic_search_service,
                &product_personalization_service,
                &upsert_service,
                &enrich_service,
                &*access_token_verifier_service,
            )
            .await
        },
    ))
    .await
}
