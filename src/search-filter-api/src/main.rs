use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use aws_sdk_dynamodb::Client;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use search_filter::service::user_search_filter_service::UserSearchFilterServiceImpl;
use search_filter_api::handler;
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
    let client = Client::new(&aws_config);

    let repository = UserSearchFilterDynamoDbRepositoryImpl::new(&client, &table_name);
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(&client, &table_name);
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(&client, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&client, &table_name);
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(&client, &table_name);

    static NOOP_SES: NoopSesAdapter = NoopSesAdapter;
    static NOOP_S3: NoopS3Adapter = NoopS3Adapter;

    let get_product_service = GetProductServiceImpl::new(&product_dynamodb_repository);
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
    );

    let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(
                event,
                &service,
                &get_product_service,
                &product_personalization_service,
            )
            .await
        },
    ))
    .await
}
