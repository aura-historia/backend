use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sesv2::Client as SesClient;
use aws_smithy_types::timeout::TimeoutConfig;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification_delivery_lambda::{
    compose_delivery_use_case, handler_with_invocation_budget, invocation_budget,
    retain_all_records,
};
use notification_email_aws::EmailDeliveryConfig;
use notification_service::use_cases::commands::deliver_notification::DeliverNotificationUseCase;
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease, log_cold_start,
    log_invocation_start, logging_config_from_env,
};
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use std::{future::Future, sync::Arc, time::Instant};
use tracing::warn;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let email = email_config()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .timeout_config(
            TimeoutConfig::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .operation_attempt_timeout(std::time::Duration::from_secs(20))
                .operation_timeout(std::time::Duration::from_secs(30))
                .build(),
        )
        .load()
        .await;
    let s3 = S3Client::new(&aws_config);
    let ses = SesClient::new(&aws_config);
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start("notification-delivery-lambda", initialization_started_at);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let email = email.clone();
        let credentials = Arc::clone(&credentials);
        let s3 = s3.clone();
        let ses = ses.clone();
        let use_cases = Arc::clone(&use_cases);
        async move {
            log_invocation_start("notification-delivery-lambda", &event.context);
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
                        compose_delivery_use_case(pool, s3, ses, email)
                    })
                    .await
            })
            .await
        }
    }))
    .await
}

type DeliveryUseCase = Arc<dyn DeliverNotificationUseCase>;
type DeliveryUseCaseLease = VersionedCompositionLease<DeliveryUseCase>;

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<DeliveryUseCaseLease, Error>>,
{
    let budget = invocation_budget(&event.context);
    let Some(use_case) = complete_before_invocation_deadline(&budget, setup()).await else {
        warn!(
            outcome = "invocation_setup_timeout",
            "Notification delivery setup retained every record for SQS retry or redrive"
        );
        return retain_all_records(&event);
    };
    let Ok(use_case) = use_case else {
        warn!(
            outcome = "invocation_setup_failed",
            "Notification delivery setup retained every record for SQS retry or redrive"
        );
        return retain_all_records(&event);
    };
    handler_with_invocation_budget(event, use_case.value().as_ref(), &budget).await
}

fn email_config() -> Result<EmailDeliveryConfig, Error> {
    Ok(EmailDeliveryConfig::new(
        required_env("S3_BUCKET_NAME_TEMPLATES")?,
        required_env("NOTIFICATION_EMAIL_FROM")?,
        required_env("NOTIFICATION_EMAIL_REPLY_TO")?,
        required_env("STAGE")?,
        required_env("COMMIT_SHA")?,
    ))
}

fn required_env(name: &'static str) -> Result<String, Error> {
    std::env::var(name).map_err(|_| Error::from(format!("missing required {name}")))
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
