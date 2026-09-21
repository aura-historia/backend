use aws_lambda_events::eventbridge::EventBridgeEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, log_cold_start, log_invocation_start,
    logging_config_from_env, required_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use serde_json::Value;
use std::{sync::Arc, time::Instant};
use stripe_lambda::{StripeProductTierMap, handler};
use user_postgres::{SqlxUserRepositoryFactory, SqlxUserTierEntitlementsFactory};
use user_service::use_cases::ApplyStripeSubscriptionHandler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let tier_map = StripeProductTierMap {
        pro_product_listing_id: required_config_from_env("STRIPE_PRO_PRODUCT_ID")?,
        ultimate_product_listing_id: required_config_from_env("STRIPE_ULTIMATE_PRODUCT_ID")?,
    };
    let subscriptions = Arc::new(VersionedCompositionCache::new());

    log_cold_start("stripe-lambda", initialization_started_at);

    run(service_fn(
        move |event: LambdaEvent<EventBridgeEvent<Value>>| {
            let postgres = postgres.clone();
            let credentials = Arc::clone(&credentials);
            let tier_map = tier_map.clone();
            let subscriptions = Arc::clone(&subscriptions);
            async move {
                log_invocation_start("stripe-lambda", &event.context);
                let credentials = credentials
                    .current()
                    .await
                    .map_err(|_| Error::from("PostgreSQL credential refresh unavailable"))?;
                let subscriptions = subscriptions
                    .get_or_try_build(credentials.version_id(), || async {
                        let pool = postgres
                            .pool_config(credentials.credentials())
                            .map_err(|_| Error::from("invalid PostgreSQL configuration"))?
                            .connect()
                            .await
                            .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
                        Ok::<_, Error>(ApplyStripeSubscriptionHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxUserRepositoryFactory::new(),
                            SqlxUserTierEntitlementsFactory::new(),
                        ))
                    })
                    .await?;

                handler(event, subscriptions.value(), &tier_map).await
            }
        },
    ))
    .await
}
