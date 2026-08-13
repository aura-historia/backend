use aws_lambda_events::eventbridge::EventBridgeEvent;
use common::postgres::SqlxUnitOfWork;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use serde_json::Value;
use stripe_lambda::{StripeProductTierMap, handler};
use user_postgres::{SqlxUserRepositoryFactory, SqlxUserTierEntitlementsFactory};
use user_service::use_cases::ApplyStripeSubscriptionHandler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let pool = common::postgres::connect_from_env().await?;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let pro_product_id = std::env::var("STRIPE_PRO_PRODUCT_ID")
        .expect("shouldn't fail loading env-var 'STRIPE_PRO_PRODUCT_ID'");
    let ultimate_product_id = std::env::var("STRIPE_ULTIMATE_PRODUCT_ID")
        .expect("shouldn't fail loading env-var 'STRIPE_ULTIMATE_PRODUCT_ID'");

    let subscriptions = ApplyStripeSubscriptionHandler::new(
        unit_of_work,
        SqlxUserRepositoryFactory::new(),
        SqlxUserTierEntitlementsFactory::new(),
    );
    let tier_map = StripeProductTierMap {
        pro_product_id,
        ultimate_product_id,
    };

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<EventBridgeEvent<Value>>| async {
            handler(event, &subscriptions, &tier_map).await
        },
    ))
    .await
}
