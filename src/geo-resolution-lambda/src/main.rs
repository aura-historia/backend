use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use geo_amazon_location::{AmazonLocationConfig, AmazonLocationGeocoder, REGION, TERMS_PROFILE};
use geo_postgres::SqlxResolutionRepositoryFactory;
use geo_service::{
    geocoding::ResultRetention,
    resolution::{ResolveHandler, ResolveUseCase},
};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaInvocationBudget, LambdaPostgresConfig, VersionedCompositionCache, log_cold_start,
    log_invocation_start, logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
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
    let config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .region(aws_config::Region::new(REGION))
        .load()
        .await;
    // One SDK attempt per budget reservation; durable retries own the charging bound.
    let provider = Arc::new(AmazonLocationGeocoder::new(
        aws_sdk_geoplaces::Client::new(&config),
        AmazonLocationConfig::new(Duration::from_secs(6), Duration::from_secs(2), 1)?
            .with_retention(ResultRetention::Storage),
    )?);
    let cache = Arc::new(VersionedCompositionCache::<Arc<dyn ResolveUseCase>>::new());
    log_cold_start("geo-resolution-lambda", started);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let credentials = Arc::clone(&credentials);
        let provider = Arc::clone(&provider);
        let cache = Arc::clone(&cache);
        async move {
            log_invocation_start("geo-resolution-lambda", &event.context);
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
                        Ok::<_, Error>(Arc::new(ResolveHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxResolutionRepositoryFactory,
                            provider,
                            TERMS_PROFILE.into(),
                        )) as Arc<dyn ResolveUseCase>)
                    })
                    .await?;
                geo_resolution_lambda::handler(event, use_case.value().as_ref(), budget.remaining())
                    .await
            })
            .await
            .map_err(|_| Error::from("geographic resolution invocation timed out"))?
        }
    }))
    .await
}
