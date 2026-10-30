use aws_lambda_events::eventbridge::EventBridgeEvent;
use billing_stripe::StripeSubscriptionReader;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, current_postgres_credentials, log_cold_start,
    log_invocation_start, logging_config_from_env, required_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
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

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let provider = StripeSubscriptionReader::new(
        required_config_from_env("STRIPE_API_KEY")?,
        required_config_from_env("STRIPE_PRO_PRODUCT_ID")?,
        required_config_from_env("STRIPE_ULTIMATE_PRODUCT_ID")?,
    )?;
    let subscriptions = Arc::new(VersionedCompositionCache::new());

    log_cold_start(COMPONENT, initialization_started_at);

    run(service_fn(
        move |event: LambdaEvent<EventBridgeEvent<Value>>| {
            let postgres = postgres.clone();
            let credentials = Arc::clone(&credentials);
            let provider = provider.clone();
            let subscriptions = Arc::clone(&subscriptions);
            async move {
                log_invocation_start(COMPONENT, &event.context);
                let credentials =
                    current_postgres_credentials(COMPONENT, credentials.as_ref()).await?;
                let subscriptions = subscriptions
                    .get_or_try_build(credentials.version_id(), || async {
                        let pool = postgres
                            .connect(COMPONENT, credentials.credentials())
                            .await?;
                        Ok::<_, Error>(ApplyStripeSubscriptionHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxUserRepositoryFactory::new(),
                            SqlxUserTierEntitlementsFactory::new(),
                            SqlxStripeSubscriptionEventStoreFactory,
                            provider,
                        ))
                    })
                    .await?;

                handler(event, subscriptions.value()).await
            }
        },
    ))
    .await
}
