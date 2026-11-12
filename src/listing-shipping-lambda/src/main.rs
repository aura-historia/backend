use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaInvocationBudget, LazyComposition, RuntimePostgresConfig, log_cold_start,
    log_invocation_start,
};
use platform_observability::{LoggingConfig, init};
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::shipping_projection::SqlxShippingProjectionStoreFactory;
use product_listing_service::shipping_projection::{
    ProjectListingShippingHandler, ProjectListingShippingUseCase,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const COMPONENT: &str = "listing-shipping-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let started = Instant::now();
    init(LoggingConfig::from_env());
    let postgres = RuntimePostgresConfig::from_env()?;
    let cache = Arc::new(LazyComposition::<Arc<dyn ProjectListingShippingUseCase>>::new());
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
                        Ok::<_, Error>(Arc::new(ProjectListingShippingHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxShippingProjectionStoreFactory,
                        ))
                            as Arc<dyn ProjectListingShippingUseCase>)
                    })
                    .await?;
                listing_shipping_lambda::handler(event, use_case.as_ref(), budget.remaining()).await
            })
            .await
            .map_err(|_| Error::from("listing shipping invocation timed out"))?
        }
    }))
    .await
}
