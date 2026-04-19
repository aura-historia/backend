use aws_config::BehaviorVersion;
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_sdk_dynamodb::Client;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use serde_json::Value;
use stripe_lambda::{StripeProductTierMap, handler};
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
    let pro_product_id = std::env::var("STRIPE_PRO_PRODUCT_ID")
        .expect("shouldn't fail loading env-var 'STRIPE_PRO_PRODUCT_ID'");
    let ultimate_product_id = std::env::var("STRIPE_ULTIMATE_PRODUCT_ID")
        .expect("shouldn't fail loading env-var 'STRIPE_ULTIMATE_PRODUCT_ID'");

    let client = Client::new(&aws_config);
    let repository = UserDynamoDbRepositoryImpl::new(&client, &table_name);
    let service = UserServiceImpl::new(&repository);
    let tier_map = StripeProductTierMap {
        pro_product_id,
        ultimate_product_id,
    };

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<EventBridgeEvent<Value>>| async {
            handler(event, &service, &tier_map).await
        },
    ))
    .await
}
