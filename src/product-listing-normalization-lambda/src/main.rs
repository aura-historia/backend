use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease, log_cold_start,
    log_invocation_start, logging_config_from_env,
};
use platform_lambda_sqs::handle_sqs_invocation;
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use product_listing_normalization_lambda::{
    compose_normalization_use_case, handler_with_invocation_budget, invocation_budget,
};
use product_service::use_cases::NormalizeProductListingRawRevisionUseCase;
use std::{future::Future, sync::Arc, time::Instant};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start(
        "product-listing-normalization-lambda",
        initialization_started_at,
    );
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let credentials = Arc::clone(&credentials);
        let use_cases = Arc::clone(&use_cases);
        async move {
            log_invocation_start("product-listing-normalization-lambda", &event.context);
            handle_invocation(event, move || async move {
                let credentials = credentials
                    .current()
                    .await
                    .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
                use_cases
                    .get_or_try_build(credentials.version_id(), || async {
                        let pool = postgres
                            .pool_config(credentials.credentials())
                            .map_err(|_| Error::from("invalid PostgreSQL configuration"))?
                            .connect()
                            .await
                            .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
                        Ok::<_, Error>(compose_normalization_use_case(pool))
                    })
                    .await
            })
            .await
        }
    }))
    .await
}

type NormalizationUseCase = Arc<dyn NormalizeProductListingRawRevisionUseCase>;
type NormalizationUseCaseLease = VersionedCompositionLease<NormalizationUseCase>;

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<NormalizationUseCaseLease, Error>>,
{
    handle_sqs_invocation(
        event,
        "product-listing-normalization-lambda",
        invocation_budget,
        setup,
        |event, use_case, budget| async move {
            handler_with_invocation_budget(event, use_case.value().as_ref(), &budget).await
        },
    )
    .await
}
