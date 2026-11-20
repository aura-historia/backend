//! `notification-delivery`: sends one claimed notification delivery through its channel.
//! PostgreSQL owns the delivery lease and finalization; the job only wakes the attempt.

use crate::composition::CompositionError;
use application::error::BoxError;
use async_trait::async_trait;
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use aws_config::BehaviorVersion;
use aws_smithy_types::timeout::TimeoutConfig;
use notification_core::notification_delivery::NotificationDeliveryChannel;
use notification_email_aws::{EmailDeliveryConfig, SesNotificationChannelSender};
use notification_postgres::{SqlxEmailDeliveryTargetReader, SqlxNotificationDeliveryRepository};
use notification_service::{
    ports::notification_channel_sender::{
        NotificationChannelSender, NotificationDeliveryDispatcher,
    },
    ports::notification_delivery_repository::NotificationDeliveryError,
    use_cases::commands::deliver_notification::{
        DeliverNotificationCommand, DeliverNotificationError, DeliverNotificationHandler,
        DeliverNotificationResult, DeliverNotificationTiming, DeliverNotificationUseCase,
        DeliveryAttemptPhase,
    },
};
use platform_postgres_queue::Disposition;
use platform_process::{Dependency, FailureDiagnostic, config::ConfigError};
use platform_worker::{Job, JobHandler, error_class};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};

const SCOPE: &str = "notification-delivery";
// Both stay below the scope's per-job timeout so a slow attempt ends through the service's own
// timeout handling, which keeps the lease consistent, rather than through cancellation.
const DELIVERY_ATTEMPT_BUDGET: Duration = Duration::from_secs(35);
const DELIVERY_OPERATION_BUDGET: Duration = Duration::from_secs(30);

pub fn compose_delivery_use_case(
    pool: PgPool,
    ses: aws_sdk_sesv2::Client,
    email: EmailDeliveryConfig,
) -> Result<Arc<dyn DeliverNotificationUseCase>, CompositionError> {
    let sender = SesNotificationChannelSender::new(
        ses,
        email,
        Arc::new(SqlxEmailDeliveryTargetReader::new(pool.clone())),
    )
    .map_err(|error| CompositionError::new(SCOPE, error))?;
    let dispatcher = NotificationDeliveryDispatcher::new(vec![
        Arc::new(sender) as Arc<dyn NotificationChannelSender>
    ])
    .map_err(|_| {
        CompositionError::new(
            SCOPE,
            "notification delivery dispatcher configuration is invalid",
        )
    })?;
    dispatcher
        .validate_channels([NotificationDeliveryChannel::Email])
        .map_err(|_| {
            CompositionError::new(SCOPE, "EMAIL notification channel is not registered")
        })?;
    let timing = DeliverNotificationTiming::new(
        DELIVERY_ATTEMPT_BUDGET,
        DELIVERY_OPERATION_BUDGET,
        Duration::from_millis(100),
        Duration::from_secs(5),
    )
    .map_err(|_| CompositionError::new(SCOPE, "notification delivery timing is invalid"))?;
    Ok(Arc::new(DeliverNotificationHandler::with_timing(
        SqlxNotificationDeliveryRepository::new(pool),
        dispatcher,
        timing,
    )))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationDeliveryJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl NotificationDeliveryJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

/// Decode and execute one compact schema-1 delivery wake-up without exposing queue transport
/// metadata to the notification service. Lease ownership and finalization remain PostgreSQL-owned.
pub async fn process_notification_delivery_job(
    body: &str,
    use_case: &(dyn DeliverNotificationUseCase + Send + Sync),
) -> NotificationDeliveryJobDisposition {
    let job = match decode(body, WorkerScope::NotificationDelivery) {
        Ok(job) => job,
        Err(_) => return NotificationDeliveryJobDisposition::Poison("invalid_wire_job"),
    };
    let command = match command_from_job(job) {
        Ok(command) => command,
        Err(_) => return NotificationDeliveryJobDisposition::Poison("unexpected_payload"),
    };
    let notification_delivery_id = command.notification_delivery_id;
    match use_case.execute(command).await {
        Ok(result) => {
            let attempt_count = match result {
                DeliverNotificationResult::Delivered { attempt_count } => Some(attempt_count),
                _ => None,
            };
            let disposition = delivery_disposition(result);
            info!(
                notification_delivery_id = %notification_delivery_id,
                attempt_count,
                outcome = disposition.category(),
                "notification delivery attempt finished"
            );
            disposition
        }
        Err(error) => {
            let failure = delivery_failure(&error);
            let code = provider_code(&error);
            let disposition = delivery_error_disposition(error);
            warn!(
                notification_delivery_id = %notification_delivery_id,
                outcome = disposition.category(),
                stage = failure.as_ref().map(FailureDiagnostic::stage),
                dependency = failure.as_ref().and_then(FailureDiagnostic::dependency),
                error_category = failure.as_ref().map(FailureDiagnostic::category),
                sqlstate = failure.as_ref().and_then(FailureDiagnostic::sqlstate),
                provider_code = code,
                "notification delivery attempt remains unfinished"
            );
            disposition
        }
    }
}

pub fn delivery_disposition(
    result: DeliverNotificationResult,
) -> NotificationDeliveryJobDisposition {
    match result {
        DeliverNotificationResult::Delivered { .. } => {
            NotificationDeliveryJobDisposition::Complete("delivered")
        }
        DeliverNotificationResult::AlreadyDelivered => {
            NotificationDeliveryJobDisposition::Complete("already_delivered")
        }
        DeliverNotificationResult::PermanentlyFailed => {
            NotificationDeliveryJobDisposition::Complete("permanently_failed")
        }
        DeliverNotificationResult::SourceMissing => {
            NotificationDeliveryJobDisposition::Complete("source_missing_finalized")
        }
        DeliverNotificationResult::DeliveryMissing => {
            NotificationDeliveryJobDisposition::Retry("delivery_missing")
        }
        DeliverNotificationResult::AlreadyClaimed { .. } => {
            NotificationDeliveryJobDisposition::Retry("already_claimed")
        }
        DeliverNotificationResult::ClaimDeferred { .. } => {
            NotificationDeliveryJobDisposition::Retry("claim_deferred")
        }
    }
}

pub fn delivery_error_disposition(
    error: DeliverNotificationError,
) -> NotificationDeliveryJobDisposition {
    match error {
        DeliverNotificationError::Repository(
            NotificationDeliveryError::InvalidPersistedState { .. },
        ) => NotificationDeliveryJobDisposition::Poison("delivery_state_invalid"),
        DeliverNotificationError::UnregisteredChannel { .. } => {
            NotificationDeliveryJobDisposition::Poison("delivery_channel_unregistered")
        }
        DeliverNotificationError::LeaseLost => {
            NotificationDeliveryJobDisposition::Retry("lease_lost")
        }
        DeliverNotificationError::AmbiguousSend(_) => {
            NotificationDeliveryJobDisposition::DependencyUnavailable("provider_acceptance_unknown")
        }
        DeliverNotificationError::AttemptTimedOut { .. } => {
            NotificationDeliveryJobDisposition::DependencyUnavailable("delivery_attempt_timeout")
        }
        DeliverNotificationError::FinalizationExhausted { .. } => {
            NotificationDeliveryJobDisposition::DependencyUnavailable(
                "delivery_finalization_unconfirmed",
            )
        }
        DeliverNotificationError::Repository(NotificationDeliveryError::OperationFailed {
            ..
        })
        | DeliverNotificationError::RetryableSend(_) => {
            NotificationDeliveryJobDisposition::DependencyUnavailable(
                "delivery_dependency_unavailable",
            )
        }
    }
}

/// The channel adapter's own static code for a failed send, logged beside the shared category.
fn provider_code(error: &DeliverNotificationError) -> Option<&'static str> {
    match error {
        DeliverNotificationError::RetryableSend(send)
        | DeliverNotificationError::AmbiguousSend(send) => Some(send.code()),
        _ => None,
    }
}

/// Safe cause facts for dependency failures; the error chain itself is never logged. Lease,
/// channel-registration and invalid-state outcomes are fully named by their category.
fn delivery_failure(error: &DeliverNotificationError) -> Option<FailureDiagnostic> {
    match error {
        DeliverNotificationError::Repository(NotificationDeliveryError::OperationFailed {
            ..
        }) => Some(FailureDiagnostic::postgres("delivery_repository", error)),
        DeliverNotificationError::FinalizationExhausted { .. } => {
            Some(FailureDiagnostic::postgres("finalization", error))
        }
        DeliverNotificationError::RetryableSend(_) => Some(FailureDiagnostic::new(
            "provider_send",
            Dependency::NotificationChannel,
            "retryable",
            None,
        )),
        DeliverNotificationError::AmbiguousSend(_) => Some(FailureDiagnostic::new(
            "provider_send",
            Dependency::NotificationChannel,
            "acceptance_unknown",
            None,
        )),
        DeliverNotificationError::AttemptTimedOut { phase } => {
            let (stage, dependency) = match phase {
                DeliveryAttemptPhase::Claim => ("claim", Dependency::Postgres),
                DeliveryAttemptPhase::Provider => {
                    ("provider_send", Dependency::NotificationChannel)
                }
                DeliveryAttemptPhase::Finalization => ("finalization", Dependency::Postgres),
            };
            Some(FailureDiagnostic::new(stage, dependency, "timeout", None))
        }
        DeliverNotificationError::Repository(
            NotificationDeliveryError::InvalidPersistedState { .. },
        )
        | DeliverNotificationError::UnregisteredChannel { .. }
        | DeliverNotificationError::LeaseLost => None,
    }
}

pub fn command_from_job(
    job: DomainJob,
) -> Result<DeliverNotificationCommand, aura_historia_jobs::InvalidJob> {
    let DomainJobPayload::NotificationDeliveryCreated(delivery) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(DeliverNotificationCommand {
        notification_delivery_id: delivery.notification_delivery_id,
    })
}

/// Only a durable terminal delivery result acknowledges; every other outcome, including poison,
/// is retried so the queue's attempt bound, not the handler, decides failure custody.
pub fn disposition(job: &Job, outcome: NotificationDeliveryJobDisposition) -> Disposition {
    if let NotificationDeliveryJobDisposition::Complete(_) = outcome {
        return Disposition::Complete;
    }
    let category = outcome.category();
    warn!(
        job_id = %job.id(),
        attempt = job.attempt(),
        outcome = category,
        "Notification delivery record retained for retry"
    );
    Disposition::Retry(error_class(category))
}

pub struct NotificationDeliveryJobs {
    use_case: Arc<dyn DeliverNotificationUseCase>,
}

impl NotificationDeliveryJobs {
    pub fn new(use_case: Arc<dyn DeliverNotificationUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for NotificationDeliveryJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let outcome =
            process_notification_delivery_job(job.payload(), self.use_case.as_ref()).await;
        disposition(job, outcome)
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    let email = email_config_from(platform_process::config::required)
        .map_err(|error| CompositionError::new(SCOPE, error))?;
    let aws_config = aws_config::defaults(BehaviorVersion::v2026_11_17())
        .timeout_config(
            TimeoutConfig::builder()
                .connect_timeout(Duration::from_secs(5))
                .operation_attempt_timeout(Duration::from_secs(20))
                .operation_timeout(Duration::from_secs(30))
                .build(),
        )
        .load()
        .await;
    let use_case =
        compose_delivery_use_case(pool.clone(), aws_sdk_sesv2::Client::new(&aws_config), email)?;
    Ok(Arc::new(NotificationDeliveryJobs::new(use_case)))
}

/// Errors name the setting, never its value.
fn email_config_from(
    setting: impl Fn(&str) -> Result<String, ConfigError>,
) -> Result<EmailDeliveryConfig, BoxError> {
    Ok(EmailDeliveryConfig::new(
        setting("MAIL_TEMPLATE_DIR")?,
        setting("NOTIFICATION_EMAIL_FROM")?,
        setting("NOTIFICATION_EMAIL_REPLY_TO")?,
        setting("NOTIFICATION_EMAIL_CONFIGURATION_SET")?,
        &setting("NOTIFICATION_EMAIL_FRONTEND_ORIGIN")?,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
    use notification_service::ports::notification_channel_sender::NotificationChannelSendError;
    use std::{collections::VecDeque, sync::Mutex};
    use test_api::CapturedLogs;

    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    #[tokio::test]
    async fn should_distinguish_repository_and_provider_failures_without_logging_causes() {
        let handler = NotificationDeliveryJobs::new(Arc::new(FakeUseCase::new([
            Err(DeliverNotificationError::Repository(
                NotificationDeliveryError::OperationFailed {
                    source: Box::new(sqlx::Error::Io(std::io::Error::other(SENTINEL))),
                },
            )),
            Err(DeliverNotificationError::RetryableSend(
                NotificationChannelSendError::Retryable {
                    code: "THROTTLED",
                    source: Box::new(std::io::Error::other(SENTINEL)),
                },
            )),
        ])));
        let (logs, guard) = CapturedLogs::start();

        let repository = handler.handle(&claimed_job(&valid_body())).await;
        let provider = handler.handle(&claimed_job(&valid_body())).await;
        drop(guard);

        let unavailable = Disposition::Retry(error_class("delivery_dependency_unavailable"));
        assert_eq!(unavailable, repository);
        assert_eq!(unavailable, provider);
        let failures = logs.events_where(
            "message",
            "notification delivery attempt remains unfinished",
        );
        let diagnostics = failures
            .iter()
            .map(|fields| {
                (
                    fields["stage"].as_str(),
                    fields["dependency"].as_str(),
                    fields["error_category"].as_str(),
                    fields.get("provider_code").and_then(|code| code.as_str()),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            diagnostics,
            [
                (
                    Some("delivery_repository"),
                    Some("postgres"),
                    Some("io"),
                    None
                ),
                (
                    Some("provider_send"),
                    Some("notification_channel"),
                    Some("retryable"),
                    Some("THROTTLED")
                ),
            ]
        );
        assert!(!logs.text().contains("SENTINEL"));
    }

    #[tokio::test]
    async fn acknowledges_only_durable_terminal_delivery_results() {
        let cases: Vec<(
            Result<DeliverNotificationResult, DeliverNotificationError>,
            Disposition,
        )> = vec![
            (
                Ok(DeliverNotificationResult::Delivered { attempt_count: 1 }),
                Disposition::Complete,
            ),
            (
                Ok(DeliverNotificationResult::AlreadyDelivered),
                Disposition::Complete,
            ),
            (
                Ok(DeliverNotificationResult::PermanentlyFailed),
                Disposition::Complete,
            ),
            (
                Ok(DeliverNotificationResult::SourceMissing),
                Disposition::Complete,
            ),
            (
                Ok(DeliverNotificationResult::DeliveryMissing),
                Disposition::Retry(error_class("delivery_missing")),
            ),
            (
                Ok(DeliverNotificationResult::AlreadyClaimed {
                    lease_expires_at: time::OffsetDateTime::now_utc(),
                }),
                Disposition::Retry(error_class("already_claimed")),
            ),
            (
                Ok(DeliverNotificationResult::ClaimDeferred {
                    retry_after: Duration::from_secs(1),
                }),
                Disposition::Retry(error_class("claim_deferred")),
            ),
            (
                Err(DeliverNotificationError::LeaseLost),
                Disposition::Retry(error_class("lease_lost")),
            ),
            (
                Err(DeliverNotificationError::Repository(
                    NotificationDeliveryError::InvalidPersistedState {
                        source: Box::new(std::io::Error::other(SENTINEL)),
                    },
                )),
                Disposition::Retry(error_class("delivery_state_invalid")),
            ),
            (
                Err(DeliverNotificationError::UnregisteredChannel {
                    channel: NotificationDeliveryChannel::Email,
                }),
                Disposition::Retry(error_class("delivery_channel_unregistered")),
            ),
            (
                Err(DeliverNotificationError::AmbiguousSend(
                    NotificationChannelSendError::Ambiguous {
                        code: "TIMEOUT",
                        source: Box::new(std::io::Error::other(SENTINEL)),
                    },
                )),
                Disposition::Retry(error_class("provider_acceptance_unknown")),
            ),
            (
                Err(DeliverNotificationError::AttemptTimedOut {
                    phase: DeliveryAttemptPhase::Provider,
                }),
                Disposition::Retry(error_class("delivery_attempt_timeout")),
            ),
            (
                Err(DeliverNotificationError::FinalizationExhausted {
                    source: NotificationDeliveryError::OperationFailed {
                        source: Box::new(std::io::Error::other(SENTINEL)),
                    },
                }),
                Disposition::Retry(error_class("delivery_finalization_unconfirmed")),
            ),
            (
                Err(DeliverNotificationError::RetryableSend(
                    NotificationChannelSendError::Retryable {
                        code: "THROTTLED",
                        source: Box::new(std::io::Error::other(SENTINEL)),
                    },
                )),
                Disposition::Retry(error_class("delivery_dependency_unavailable")),
            ),
        ];
        let (results, expected): (Vec<_>, Vec<_>) = cases.into_iter().unzip();
        let handler = NotificationDeliveryJobs::new(Arc::new(FakeUseCase::new(results)));
        let mut actual = Vec::new();
        for _ in 0..expected.len() {
            actual.push(handler.handle(&claimed_job(&valid_body())).await);
        }
        assert_eq!(expected, actual);
    }

    #[tokio::test]
    async fn should_retry_malformed_and_foreign_scope_jobs_when_they_cannot_be_decoded() {
        let handler = NotificationDeliveryJobs::new(Arc::new(FakeUseCase::new([])));
        assert_eq!(
            Disposition::Retry(error_class("invalid_wire_job")),
            handler.handle(&claimed_job("not-json")).await
        );
        let other_payload = valid_body().replace(
            r#""scope":"notification-delivery""#,
            r#""scope":"watchlist-notification""#,
        );
        assert_eq!(
            Disposition::Retry(error_class("invalid_wire_job")),
            handler.handle(&claimed_job(&other_payload)).await
        );
    }

    #[tokio::test]
    async fn should_log_the_retained_job_by_id_without_its_payload() {
        let handler = NotificationDeliveryJobs::new(Arc::new(FakeUseCase::new([Ok(
            DeliverNotificationResult::DeliveryMissing,
        )])));
        let job = claimed_job(&valid_body());
        let (logs, guard) = CapturedLogs::start();

        handler.handle(&job).await;
        drop(guard);

        let retained =
            logs.events_where("message", "Notification delivery record retained for retry");
        assert_eq!(1, retained.len());
        assert_eq!(
            Some(job.id().to_string().as_str()),
            retained[0]["job_id"].as_str()
        );
        assert_eq!(Some("delivery_missing"), retained[0]["outcome"].as_str());
        assert!(!logs.text().contains("idempotency_key"));
    }

    const CONFIGURATION_SET_ENV: &str = "NOTIFICATION_EMAIL_CONFIGURATION_SET";

    fn test_env(name: &str) -> Result<String, ConfigError> {
        Ok(match name {
            "MAIL_TEMPLATE_DIR" => "/usr/share/aura-historia/mail-templates",
            "NOTIFICATION_EMAIL_FROM" => "from@example.test",
            "NOTIFICATION_EMAIL_REPLY_TO" => "reply@example.test",
            CONFIGURATION_SET_ENV => "aura-historia-test-email",
            "STAGE" => "test",
            "NOTIFICATION_EMAIL_FRONTEND_ORIGIN" => "https://stage.aura-historia.com",
            _ => return Err(ConfigError::Missing { name: name.into() }),
        }
        .to_owned())
    }

    #[test]
    fn should_require_configuration_set_env_without_falling_back_to_stage() {
        let error = email_config_from(|name| {
            if name == CONFIGURATION_SET_ENV {
                Err(ConfigError::Missing { name: name.into() })
            } else {
                test_env(name)
            }
        })
        .err()
        .expect("configuration set is required");
        assert_eq!(
            "missing required configuration NOTIFICATION_EMAIL_CONFIGURATION_SET",
            error.to_string()
        );
    }

    fn valid_body() -> String {
        r#"{"schema_version":1,"scope":"notification-delivery","job_type":"NOTIFICATION_DELIVERY_CREATED","idempotency_key":"notification-delivery:nd_01j0000000e008000000000007","ordering_key":"notification-delivery:nd_01j0000000e008000000000007","payload":{"notification_delivery_id":"nd_01j0000000e008000000000007"}}"#.to_owned()
    }

    struct FakeUseCase {
        results: Mutex<VecDeque<Result<DeliverNotificationResult, DeliverNotificationError>>>,
    }

    impl FakeUseCase {
        fn new(
            results: impl IntoIterator<
                Item = Result<DeliverNotificationResult, DeliverNotificationError>,
            >,
        ) -> Self {
            Self {
                results: Mutex::new(results.into_iter().collect()),
            }
        }
    }

    #[async_trait::async_trait]
    impl DeliverNotificationUseCase for FakeUseCase {
        async fn execute(
            &self,
            _: DeliverNotificationCommand,
        ) -> Result<DeliverNotificationResult, DeliverNotificationError> {
            self.results
                .lock()
                .expect("results lock")
                .pop_front()
                .expect("unexpected use case invocation")
        }
    }
}
