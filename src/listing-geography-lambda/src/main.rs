use aws_lambda_events::sqs::SqsEvent;
use geo_postgres::SqlxResolutionRepositoryFactory;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaInvocationBudget, LazyComposition, RuntimePostgresConfig, log_cold_start,
    log_invocation_start,
};
use platform_observability::{LoggingConfig, init};
use platform_postgres::SqlxUnitOfWork;
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
    init(LoggingConfig::from_env());
    let postgres = RuntimePostgresConfig::from_env()?;
    let cache = Arc::new(LazyComposition::<Arc<dyn ProjectListingGeographyUseCase>>::new());
    log_cold_start(COMPONENT, started);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let cache = Arc::clone(&cache);
        async move {
            log_invocation_start(COMPONENT, &event.context);
            let budget = LambdaInvocationBudget::from_context(
                &event.context,
                Duration::from_secs(45),
                Duration::from_secs(5),
            );
            tokio::time::timeout(budget.remaining(), async move {
                let use_case = cache
                    .get_or_try_build(|| async {
                        let pool = postgres.connect(COMPONENT).await?;
                        Ok::<_, Error>(Arc::new(ProjectListingGeographyHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxGeographyProjectionStoreFactory,
                            SqlxResolutionRepositoryFactory,
                        ))
                            as Arc<dyn ProjectListingGeographyUseCase>)
                    })
                    .await?;
                listing_geography_lambda::handler(event, use_case.as_ref(), budget.remaining())
                    .await
            })
            .await
            .map_err(|_| Error::from("listing geography invocation timed out"))?
        }
    }))
    .await
}
