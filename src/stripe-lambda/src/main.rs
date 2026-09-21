use aws_lambda_events::eventbridge::EventBridgeEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, log_cold_start, log_invocation_start, logging_config_from_env,
    required_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use serde_json::Value;
use std::time::Instant;
use stripe_lambda::{StripeProductTierMap, handler};
use user_postgres::{SqlxUserRepositoryFactory, SqlxUserTierEntitlementsFactory};
use user_service::use_cases::ApplyStripeSubscriptionHandler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let pool = LambdaPostgresConfig::from_env()?
        .into_pool_config()
        .connect()
        .await
        .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let pro_product_listing_id = required_config_from_env("STRIPE_PRO_PRODUCT_ID")?;
    let ultimate_product_listing_id = required_config_from_env("STRIPE_ULTIMATE_PRODUCT_ID")?;

    let subscriptions = ApplyStripeSubscriptionHandler::new(
        unit_of_work,
        SqlxUserRepositoryFactory::new(),
        SqlxUserTierEntitlementsFactory::new(),
    );
    let tier_map = StripeProductTierMap {
        pro_product_listing_id,
        ultimate_product_listing_id,
    };

    log_cold_start("stripe-lambda", initialization_started_at);

    run(service_fn(
        |event: LambdaEvent<EventBridgeEvent<Value>>| async {
            log_invocation_start("stripe-lambda", &event.context);
            handler(event, &subscriptions, &tier_map).await
        },
    ))
    .await
}
