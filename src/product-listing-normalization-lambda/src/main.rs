use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LazyComposition, RuntimePostgresConfig, log_cold_start, log_invocation_start,
};
use platform_lambda_sqs::handle_sqs_invocation;
use platform_observability::{LoggingConfig, init};
use product_listing_normalization_lambda::{
    compose_normalization_use_case, handler_with_invocation_budget, invocation_budget,
};
use product_service::use_cases::NormalizeProductListingRawRevisionUseCase;
use std::{future::Future, sync::Arc, time::Instant};

const COMPONENT: &str = "product-listing-normalization-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(LoggingConfig::from_env());

    let postgres = RuntimePostgresConfig::from_env()?;
    let use_cases = Arc::new(LazyComposition::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let use_cases = Arc::clone(&use_cases);
        async move {
            log_invocation_start(COMPONENT, &event.context);
            handle_invocation(event, move || async move {
                use_cases
                    .get_or_try_build(|| async {
                        let pool = postgres.connect(COMPONENT).await?;
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

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<NormalizationUseCase, Error>>,
{
    handle_sqs_invocation(
        event,
        COMPONENT,
        invocation_budget,
        setup,
        |event, use_case, budget| async move {
            handler_with_invocation_budget(event, use_case.as_ref(), &budget).await
        },
    )
    .await
}
