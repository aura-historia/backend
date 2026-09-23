use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease, log_cold_start,
    log_invocation_start, logging_config_from_env,
};
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use product_listing_service::use_cases::GenerateWatchlistNotificationsUseCase;
use std::{future::Future, sync::Arc, time::Instant};
use tracing::warn;
use watchlist_notification_lambda::{
    compose_watchlist_notification_use_case, handler_with_invocation_budget, invocation_budget,
    retain_all_records,
};

const COMPONENT: &str = "watchlist-notification-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let credentials = Arc::clone(&credentials);
        let use_cases = Arc::clone(&use_cases);
        async move {
            log_invocation_start(COMPONENT, &event.context);
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
                        Ok::<_, Error>(compose_watchlist_notification_use_case(pool))
                    })
                    .await
            })
            .await
        }
    }))
    .await
}

type WatchlistNotificationUseCase = Arc<dyn GenerateWatchlistNotificationsUseCase>;
type WatchlistNotificationUseCaseLease = VersionedCompositionLease<WatchlistNotificationUseCase>;

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<WatchlistNotificationUseCaseLease, Error>>,
{
    let budget = invocation_budget(&event.context);
    let Some(use_case) = complete_before_invocation_deadline(&budget, setup()).await else {
        warn!(
            outcome = "invocation_setup_timeout",
            "Watchlist notification setup retained every record for SQS retry or redrive"
        );
        return retain_all_records(&event);
    };
    let Ok(use_case) = use_case else {
        warn!(
            outcome = "invocation_setup_failed",
            "Watchlist notification setup retained every record for SQS retry or redrive"
        );
        return retain_all_records(&event);
    };
    handler_with_invocation_budget(event, use_case.value().as_ref(), &budget).await
}

async fn complete_before_invocation_deadline<T>(
    budget: &platform_lambda_bootstrap::LambdaInvocationBudget,
    operation: impl Future<Output = T>,
) -> Option<T> {
    let remaining = budget.remaining();
    if remaining.is_zero() {
        return None;
    }
    tokio::time::timeout(remaining, operation).await.ok()
}
