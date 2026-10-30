use aws_lambda_events::sqs::SqsEvent;
use geo_postgres::SqlxResolutionRepositoryFactory;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaInvocationBudget, LambdaPostgresConfig, VersionedCompositionCache,
    current_postgres_credentials, log_cold_start, log_invocation_start, logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use product_listing_postgres::geography::SqlxGeographyProjectionStoreFactory;
use product_listing_service::geography::{
    ProjectListingGeographyHandler, ProjectListingGeographyUseCase,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const COMPONENT: &str = "listing-geography-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let started = Instant::now();
    init(logging_config_from_env());
    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credentials unavailable"))?;
    let cache = Arc::new(VersionedCompositionCache::<
        Arc<dyn ProjectListingGeographyUseCase>,
    >::new());
    log_cold_start(COMPONENT, started);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let credentials = Arc::clone(&credentials);
        let cache = Arc::clone(&cache);
        async move {
            log_invocation_start(COMPONENT, &event.context);
            let budget = LambdaInvocationBudget::from_context(
                &event.context,
                Duration::from_secs(45),
                Duration::from_secs(5),
            );
            tokio::time::timeout(budget.remaining(), async move {
                let current = current_postgres_credentials(COMPONENT, credentials.as_ref()).await?;
                let use_case = cache
                    .get_or_try_build(current.version_id(), || async {
                        let pool = postgres.connect(COMPONENT, current.credentials()).await?;
                        Ok::<_, Error>(Arc::new(ProjectListingGeographyHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxGeographyProjectionStoreFactory,
                            SqlxResolutionRepositoryFactory,
                        ))
                            as Arc<dyn ProjectListingGeographyUseCase>)
                    })
                    .await?;
                listing_geography_lambda::handler(
                    event,
                    use_case.value().as_ref(),
                    budget.remaining(),
                )
                .await
            })
            .await
            .map_err(|_| Error::from("listing geography invocation timed out"))?
        }
    }))
    .await
}
