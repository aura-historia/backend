use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaInvocationBudget, LambdaPostgresConfig, VersionedCompositionCache, log_cold_start,
    log_invocation_start, logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use shipping_postgres::projection::SqlxShippingProjectionStoreFactory;
use shipping_service::projection::{ProjectListingShippingHandler, ProjectListingShippingUseCase};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let started = Instant::now();
    init(logging_config_from_env());
    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credentials unavailable"))?;
    let cache = Arc::new(VersionedCompositionCache::<
        Arc<dyn ProjectListingShippingUseCase>,
    >::new());
    log_cold_start("listing-shipping-lambda", started);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let credentials = Arc::clone(&credentials);
        let cache = Arc::clone(&cache);
        async move {
            log_invocation_start("listing-shipping-lambda", &event.context);
            let budget = LambdaInvocationBudget::from_context(
                &event.context,
                Duration::from_secs(45),
                Duration::from_secs(5),
            );
            tokio::time::timeout(budget.remaining(), async move {
                let current = credentials
                    .current()
                    .await
                    .map_err(|_| Error::from("PostgreSQL credentials unavailable"))?;
                let use_case = cache
                    .get_or_try_build(current.version_id(), || async {
                        let pool = postgres
                            .pool_config(current.credentials())?
                            .connect()
                            .await
                            .map_err(|_| Error::from("PostgreSQL pool unavailable"))?;
                        Ok::<_, Error>(Arc::new(ProjectListingShippingHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxShippingProjectionStoreFactory,
                        ))
                            as Arc<dyn ProjectListingShippingUseCase>)
                    })
                    .await?;
                listing_shipping_lambda::handler(
                    event,
                    use_case.value().as_ref(),
                    budget.remaining(),
                )
                .await
            })
            .await
            .map_err(|_| Error::from("listing shipping invocation timed out"))?
        }
    }))
    .await
}
