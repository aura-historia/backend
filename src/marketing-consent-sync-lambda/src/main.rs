use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease, log_cold_start,
    log_invocation_start, logging_config_from_env,
};
use platform_lambda_sqs::handle_fifo_sqs_invocation;
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use std::{future::Future, sync::Arc, time::Instant};
use user_loops::LoopsNewsletterConfig;
use user_service::use_cases::SyncMarketingConsentIntentUseCase;

use marketing_consent_sync_lambda::{
    compose_marketing_consent_sync_use_case, handler_with_invocation_budget, invocation_budget,
};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let loops = loops_config()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start("marketing-consent-sync-lambda", initialization_started_at);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let loops = loops.clone();
        let credentials = Arc::clone(&credentials);
        let use_cases = Arc::clone(&use_cases);
        async move {
            log_invocation_start("marketing-consent-sync-lambda", &event.context);
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
type SyncUseCaseLease = VersionedCompositionLease<SyncUseCase>;

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<SyncUseCaseLease, Error>>,
{
    handle_fifo_sqs_invocation(
        event,
        "marketing-consent-sync-lambda",
        invocation_budget,
        setup,
        |event, use_case, budget| async move {
            handler_with_invocation_budget(event, use_case.value().as_ref(), &budget).await
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
