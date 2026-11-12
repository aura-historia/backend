use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LazyComposition, RuntimePostgresConfig, log_cold_start, log_invocation_start,
};
use platform_lambda_sqs::{SqsDeadLetterCustody, handle_fifo_sqs_invocation};
use platform_observability::{LoggingConfig, init};
use std::{future::Future, sync::Arc, time::Instant};
use user_loops::LoopsNewsletterConfig;
use user_service::use_cases::SyncMarketingConsentIntentUseCase;

use marketing_consent_sync_lambda::{
    compose_marketing_consent_sync_use_case, handler_with_invocation_budget, invocation_budget,
};

const COMPONENT: &str = "marketing-consent-sync-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(LoggingConfig::from_env());

    let postgres = RuntimePostgresConfig::from_env()?;
    let loops = loops_config()?;
    let custody = SqsDeadLetterCustody::from_env().await?;
    let use_cases = Arc::new(LazyComposition::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let loops = loops.clone();
        let use_cases = Arc::clone(&use_cases);
        let custody = custody.clone();
        async move {
            log_invocation_start(COMPONENT, &event.context);
            handle_invocation(event, &custody, move || async move {
                use_cases
                    .get_or_try_build(|| async {
                        let pool = postgres.connect(COMPONENT).await?;
                        compose_marketing_consent_sync_use_case(pool, loops)
                    })
                    .await
            })
            .await
        }
    }))
    .await
}

type SyncUseCase = Arc<dyn SyncMarketingConsentIntentUseCase>;

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<SqsEvent>,
    custody: &SqsDeadLetterCustody,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<SyncUseCase, Error>>,
{
    handle_fifo_sqs_invocation(
        event,
        COMPONENT,
        invocation_budget,
        setup,
        |event, use_case, budget| async move {
            handler_with_invocation_budget(event, use_case.as_ref(), custody, &budget).await
        },
    )
    .await
}

fn loops_config() -> Result<LoopsNewsletterConfig, Error> {
    let api_key = required_env("LOOPS_API_KEY")?;
    let list_id = required_env("LOOPS_NEWSLETTER_LIST_ID")?;
    let base_url = std::env::var("LOOPS_API_BASE_URL")
        .unwrap_or_else(|_| "https://app.loops.so/api".to_owned());
    LoopsNewsletterConfig::new(api_key, list_id, base_url)
        .map_err(|_| Error::from("Loops consent configuration is invalid"))
}

fn required_env(name: &'static str) -> Result<String, Error> {
    std::env::var(name).map_err(|_| Error::from(format!("missing required {name}")))
}
