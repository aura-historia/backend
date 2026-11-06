use aws_lambda_events::eventbridge::EventBridgeEvent;
use billing_stripe::StripeSubscriptionReader;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LazyComposition, RuntimePostgresConfig, log_cold_start, log_invocation_start,
    logging_config_from_env, required_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use serde_json::Value;
use std::{sync::Arc, time::Instant};
use stripe_lambda::handler;
use user_postgres::{
    SqlxStripeSubscriptionEventStoreFactory, SqlxUserRepositoryFactory,
    SqlxUserTierEntitlementsFactory,
};
use user_service::use_cases::ApplyStripeSubscriptionHandler;

const COMPONENT: &str = "stripe-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = RuntimePostgresConfig::from_env()?;
    let provider = StripeSubscriptionReader::new(
        required_config_from_env("STRIPE_API_KEY")?,
        required_config_from_env("STRIPE_PRO_PRODUCT_ID")?,
        required_config_from_env("STRIPE_ULTIMATE_PRODUCT_ID")?,
    )?;
    let subscriptions = Arc::new(LazyComposition::new());

    log_cold_start(COMPONENT, initialization_started_at);

    run(service_fn(
        move |event: LambdaEvent<EventBridgeEvent<Value>>| {
            let postgres = postgres.clone();
            let provider = provider.clone();
            let subscriptions = Arc::clone(&subscriptions);
            async move {
                log_invocation_start(COMPONENT, &event.context);
                let subscriptions = subscriptions
                    .get_or_try_build(|| async {
                        let pool = postgres.connect(COMPONENT).await?;
                        Ok::<_, Error>(Arc::new(ApplyStripeSubscriptionHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxUserRepositoryFactory::new(),
                            SqlxUserTierEntitlementsFactory::new(),
                            SqlxStripeSubscriptionEventStoreFactory,
                            provider,
                        )))
                    })
                    .await?;

                handler(event, subscriptions.as_ref()).await
            }
        },
    ))
    .await
}
