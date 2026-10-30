use application::operation_context::{OperationContext, Principal};
use geo_service::resolution::MaintainResolutionsUseCase;
use lambda_runtime::Error;
use oauth_service::ports::ExpiredCredentialCleanupError;
use oauth_service::use_cases::{
    CleanupExpiredCredentialsAndProviderReceiptsUseCase, ExpiryCleanupBatchSize,
    ExpiryCleanupBatchSizeError,
};
use platform_lambda_bootstrap::{Dependency, FailureDiagnostic};
use product_listing_service::geography::ProjectListingGeographyUseCase;
use product_listing_service::use_cases::{
    RefreshPublicCatalogueOverviewError, RefreshPublicCatalogueOverviewUseCase,
};
use std::env;
use std::time::Instant;
use user_service::use_cases::{CleanupConsentWorkflowError, CleanupConsentWorkflowUseCase};

pub const EXPIRY_CLEANUP_BATCH_SIZE_ENV: &str = "EXPIRY_CLEANUP_BATCH_SIZE";
/// The geocoding provider the composition wires behind resolution and geography maintenance.
const GEOCODING_PROVIDER: Option<Dependency> = Some(Dependency::AmazonLocation);
/// Runs last in the hourly maintenance invocation; the use case recalculates only once the
/// stored calculation is due, so most runs skip the full aggregate.
pub async fn run_public_catalogue_overview_refresh(
    refresh: &dyn RefreshPublicCatalogueOverviewUseCase,
) -> Result<(), Error> {
    let started_at = Instant::now();
    match refresh.execute().await {
        Ok(None) => {
            tracing::info!(
                event = "backend_cleanup.public_catalogue_overview.refreshed",
                outcome = "current",
                duration_ms = started_at.elapsed().as_millis(),
            );
            Ok(())
        }
        Ok(Some(refresh)) => {
            tracing::info!(
                event = "backend_cleanup.public_catalogue_overview.refreshed",
                outcome = "success",
                stored = refresh.stored,
                live_objects = refresh.overview.live_objects,
                sources_indexed = refresh.overview.sources_indexed,
                duration_ms = started_at.elapsed().as_millis(),
            );
            Ok(())
        }
        Err(error) => {
            let failure = overview_refresh_failure(&error);
            tracing::warn!(
                event = "backend_cleanup.public_catalogue_overview.refreshed",
                outcome = "failure",
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                sqlstate = failure.sqlstate(),
                duration_ms = started_at.elapsed().as_millis(),
            );
            Err(Error::from("public catalogue overview refresh failed"))
        }
    }
}

fn overview_refresh_failure(error: &RefreshPublicCatalogueOverviewError) -> FailureDiagnostic {
    use RefreshPublicCatalogueOverviewError as E;
    match error {
        E::RefresherTemporarilyUnavailable { .. } => {
            FailureDiagnostic::postgres("overview_calculation", error)
        }
        E::RefresherInvalidReadModel { .. } => FailureDiagnostic::new(
            "overview_calculation",
            Dependency::Postgres,
            "invalid_read_model",
            None,
        ),
        E::BeginTransaction { .. } => FailureDiagnostic::postgres("overview_begin", error),
        E::CommitTransaction { .. } => FailureDiagnostic::postgres("overview_commit", error),
    }
}

/// Runs one unscoped recovery page so geography work does not depend on wake-up delivery alone.
pub async fn run_listing_geography_recovery(
    recovery: &dyn ProjectListingGeographyUseCase,
) -> Result<(), Error> {
    let Err(error) = recovery.execute(None, 100).await else {
        return Ok(());
    };
    let failure = FailureDiagnostic::from_kind(
        error.failure_stage(),
        error.failure_kind(),
        GEOCODING_PROVIDER,
        &error,
    );
    tracing::warn!(
        event = "backend_cleanup.listing_geography.failed",
        stage = failure.stage(),
        dependency = failure.dependency(),
        error_category = failure.category(),
        sqlstate = failure.sqlstate(),
    );
    Err(Error::from(error))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CleanupConfigError {
    #[error("missing required cleanup configuration {name}")]
    Missing { name: &'static str },
    #[error("invalid cleanup configuration {name}")]
    Invalid { name: &'static str },
}

pub fn cleanup_batch_size_from_env() -> Result<ExpiryCleanupBatchSize, CleanupConfigError> {
    cleanup_batch_size_from_result(env::var(EXPIRY_CLEANUP_BATCH_SIZE_ENV))
}

fn cleanup_batch_size_from_result(
    value: Result<String, env::VarError>,
) -> Result<ExpiryCleanupBatchSize, CleanupConfigError> {
    let value = match value {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) | Err(env::VarError::NotPresent) => {
            return Err(CleanupConfigError::Missing {
                name: EXPIRY_CLEANUP_BATCH_SIZE_ENV,
            });
        }
        Err(env::VarError::NotUnicode(_)) => {
            return Err(CleanupConfigError::Invalid {
                name: EXPIRY_CLEANUP_BATCH_SIZE_ENV,
            });
        }
    };
    let value = value
        .parse::<i32>()
        .map_err(|_| CleanupConfigError::Invalid {
            name: EXPIRY_CLEANUP_BATCH_SIZE_ENV,
        })?;
    ExpiryCleanupBatchSize::try_from(value).map_err(|_: ExpiryCleanupBatchSizeError| {
        CleanupConfigError::Invalid {
            name: EXPIRY_CLEANUP_BATCH_SIZE_ENV,
        }
    })
}

pub async fn run_cleanup(
    cleanup: &(dyn CleanupExpiredCredentialsAndProviderReceiptsUseCase + Send + Sync),
    consent_cleanup: &(dyn CleanupConsentWorkflowUseCase + Send + Sync),
    geography_cleanup: &dyn MaintainResolutionsUseCase,
    batch_size: ExpiryCleanupBatchSize,
) -> Result<(), Error> {
    let started_at = Instant::now();
    let credential_result = cleanup.execute(batch_size).await;
    match &credential_result {
        Ok(counts) => {
            tracing::info!(
                event = "backend_cleanup.expired_credentials.completed",
                outcome = "success",
                batch_size = batch_size.get(),
                access_tokens_deleted = counts.access_tokens_deleted,
                authorization_codes_deleted = counts.authorization_codes_deleted,
                third_party_exchange_codes_deleted = counts.third_party_exchange_codes_deleted,
                provider_receipts_deleted = counts.provider_receipts_deleted,
                duration_ms = started_at.elapsed().as_millis(),
            );
        }
        Err(error) => {
            let failure = expired_credential_failure(error);
            tracing::warn!(
                event = "backend_cleanup.expired_credentials.completed",
                outcome = "failure",
                batch_size = batch_size.get(),
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                sqlstate = failure.sqlstate(),
                duration_ms = started_at.elapsed().as_millis(),
            );
        }
    }
    let consent_result = consent_cleanup.execute(batch_size.get() as u16).await;
    match &consent_result {
        Ok(counts) => tracing::info!(
            event = "backend_cleanup.consent_workflow.completed",
            outcome = "success",
            batch_size = batch_size.get(),
            unconfirmed_challenges_deleted = counts.unconfirmed_challenges_deleted,
            confirmed_challenges_deleted = counts.confirmed_challenges_deleted,
            completed_intents_deleted = counts.completed_intents_deleted,
            webhook_receipts_deleted = counts.webhook_receipts_deleted,
            duration_ms = started_at.elapsed().as_millis(),
        ),
        Err(error) => {
            let failure = consent_cleanup_failure(error);
            tracing::warn!(
                event = "backend_cleanup.consent_workflow.completed",
                outcome = "failure",
                batch_size = batch_size.get(),
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                sqlstate = failure.sqlstate(),
                duration_ms = started_at.elapsed().as_millis(),
            )
        }
    }
    let context = OperationContext {
        principal: Principal::Service("backend-cleanup-worker".into()),
        request_id: "backend-cleanup".into(),
        correlation_id: "backend-cleanup".into(),
    };
    let geography_limit = batch_size.get().min(100) as u32;
    let geography_result = geography_cleanup.execute(&context, geography_limit).await;
    match &geography_result {
        Ok(counts) => tracing::info!(
            event = "backend_cleanup.geographic_resolution.completed",
            outcome = "success",
            batch_size = geography_limit,
            expired = counts.expired,
            requeued = counts.requeued,
            duration_ms = started_at.elapsed().as_millis(),
        ),
        Err(error) => {
            let failure = FailureDiagnostic::from_kind(
                "resolution_maintenance",
                error.failure_kind(),
                GEOCODING_PROVIDER,
                error,
            );
            tracing::warn!(
                event = "backend_cleanup.geographic_resolution.completed",
                outcome = "failure",
                batch_size = geography_limit,
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                sqlstate = failure.sqlstate(),
                duration_ms = started_at.elapsed().as_millis(),
            )
        }
    }
    if credential_result.is_err() || consent_result.is_err() || geography_result.is_err() {
        Err(Error::from("backend cleanup failed"))
    } else {
        Ok(())
    }
}

fn expired_credential_failure(error: &ExpiredCredentialCleanupError) -> FailureDiagnostic {
    match error {
        ExpiredCredentialCleanupError::TemporarilyUnavailable { .. } => {
            FailureDiagnostic::postgres("expired_credentials", error)
        }
    }
}

fn consent_cleanup_failure(error: &CleanupConsentWorkflowError) -> FailureDiagnostic {
    match error {
        CleanupConsentWorkflowError::TransactionFailed { target, .. }
        | CleanupConsentWorkflowError::TemporarilyUnavailable { target, .. } => {
            FailureDiagnostic::postgres(target.label(), error)
        }
        CleanupConsentWorkflowError::InvalidBatchSize => {
            FailureDiagnostic::rejected("consent_workflow", "invalid_batch_size")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{error::box_error, transaction::TransactionError};
    use geo_service::resolution::{ResolutionError, ResolutionMaintenanceCounts};
    use oauth_service::ports::ExpiredCredentialCleanupError;
    use oauth_service::use_cases::CleanupCounts;
    use product_listing_service::geography::{GeographyError, GeographyWorkId};
    use product_listing_service::use_cases::{
        PublicCatalogueOverview, PublicCatalogueOverviewRefresh,
        RefreshPublicCatalogueOverviewError,
    };
    use serde_json::{Map, Value, json};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_api::CapturedLogs;
    use user_service::use_cases::{
        CleanupConsentWorkflowError, ConsentWorkflowCleanupCounts, ConsentWorkflowCleanupTarget,
    };

    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    /// The safe diagnostic fields of the one event named `event`.
    fn failure_diagnostic(logs: &CapturedLogs, event: &str) -> Value {
        let events = logs.events_where("event", event);
        assert_eq!(1, events.len(), "{event}");
        Value::Object(
            events[0]
                .iter()
                .filter(|(name, _)| {
                    [
                        "stage",
                        "dependency",
                        "error_category",
                        "http_status",
                        "sqlstate",
                    ]
                    .contains(&name.as_str())
                })
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<Map<_, _>>(),
        )
    }

    struct RecordingOverviewRefresh {
        calls: AtomicUsize,
        due: bool,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl RefreshPublicCatalogueOverviewUseCase for RecordingOverviewRefresh {
        async fn execute(
            &self,
        ) -> Result<Option<PublicCatalogueOverviewRefresh>, RefreshPublicCatalogueOverviewError>
        {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                return Err(
                    RefreshPublicCatalogueOverviewError::RefresherTemporarilyUnavailable {
                        source: Box::new(sqlx::Error::Io(std::io::Error::other(SENTINEL))),
                    },
                );
            }
            Ok(self.due.then(|| PublicCatalogueOverviewRefresh {
                overview: PublicCatalogueOverview {
                    as_of: std::time::SystemTime::UNIX_EPOCH.into(),
                    sources_indexed: 1,
                    live_objects: 2,
                    countries: 0,
                    languages: 1,
                    new_objects_last_7_days: 0,
                },
                stored: true,
            }))
        }
    }

    #[tokio::test]
    async fn should_fail_invocation_only_for_a_failed_overview_refresh() {
        for (due, fail) in [(true, false), (false, false), (true, true)] {
            let refresh = RecordingOverviewRefresh {
                calls: AtomicUsize::new(0),
                due,
                fail,
            };

            let result = run_public_catalogue_overview_refresh(&refresh).await;

            assert_eq!(fail, result.is_err());
            assert_eq!(1, refresh.calls.load(Ordering::Relaxed));
        }
    }

    #[tokio::test]
    async fn should_record_the_failed_overview_refresh_stage_without_its_cause() {
        let refresh = RecordingOverviewRefresh {
            calls: AtomicUsize::new(0),
            due: true,
            fail: true,
        };
        let (logs, guard) = CapturedLogs::start();

        let result = run_public_catalogue_overview_refresh(&refresh).await;
        drop(guard);

        assert!(result.is_err());
        assert_eq!(
            json!({"stage": "overview_calculation", "dependency": "postgres",
                "error_category": "io"}),
            failure_diagnostic(&logs, "backend_cleanup.public_catalogue_overview.refreshed")
        );
        assert!(!logs.text().contains("SENTINEL"));
    }

    struct FailingGeographyRecovery;

    #[async_trait::async_trait]
    impl ProjectListingGeographyUseCase for FailingGeographyRecovery {
        async fn execute(
            &self,
            work: Option<GeographyWorkId>,
            _limit: u16,
        ) -> Result<u32, GeographyError> {
            assert!(work.is_none());
            Err(GeographyError::Transaction(TransactionError::CommitFailed(
                box_error(sqlx::Error::Io(std::io::Error::other(SENTINEL))),
            )))
        }
    }

    #[tokio::test]
    async fn should_record_and_return_a_failed_listing_geography_recovery() {
        let (logs, guard) = CapturedLogs::start();

        let result = run_listing_geography_recovery(&FailingGeographyRecovery).await;
        drop(guard);

        assert!(result.is_err());
        assert_eq!(
            json!({"stage": "geography_transaction", "dependency": "postgres",
                "error_category": "io"}),
            failure_diagnostic(&logs, "backend_cleanup.listing_geography.failed")
        );
        assert!(!logs.text().contains("SENTINEL"));
    }

    struct RecordingCleanup {
        calls: AtomicUsize,
        fail: bool,
    }

    struct RecordingConsentCleanup {
        calls: AtomicUsize,
        fail: bool,
    }

    struct RecordingGeographyCleanup {
        calls: AtomicUsize,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl MaintainResolutionsUseCase for RecordingGeographyCleanup {
        async fn execute(
            &self,
            context: &OperationContext,
            limit: u32,
        ) -> Result<ResolutionMaintenanceCounts, ResolutionError> {
            assert!(matches!(context.principal, Principal::Service(_)));
            assert_eq!(100, limit);
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                return Err(ResolutionError::Storage(Box::new(
                    sqlx::Error::PoolTimedOut,
                )));
            }
            Ok(ResolutionMaintenanceCounts {
                expired: 1,
                requeued: 2,
            })
        }
    }

    #[async_trait::async_trait]
    impl CleanupExpiredCredentialsAndProviderReceiptsUseCase for RecordingCleanup {
        async fn execute(
            &self,
            _batch_size: ExpiryCleanupBatchSize,
        ) -> Result<CleanupCounts, ExpiredCredentialCleanupError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                return Err(ExpiredCredentialCleanupError::TemporarilyUnavailable {
                    source: Box::new(sqlx::Error::Protocol(SENTINEL.to_owned())),
                });
            }
            Ok(CleanupCounts {
                access_tokens_deleted: 1,
                authorization_codes_deleted: 2,
                third_party_exchange_codes_deleted: 3,
                provider_receipts_deleted: 4,
            })
        }
    }

    #[async_trait::async_trait]
    impl CleanupConsentWorkflowUseCase for RecordingConsentCleanup {
        async fn execute(
            &self,
            _batch_size: u16,
        ) -> Result<ConsentWorkflowCleanupCounts, CleanupConsentWorkflowError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                return Err(CleanupConsentWorkflowError::TemporarilyUnavailable {
                    target: ConsentWorkflowCleanupTarget::WebhookReceipts,
                    source: Box::new(sqlx::Error::Tls(Box::new(std::io::Error::other(SENTINEL)))),
                });
            }
            Ok(ConsentWorkflowCleanupCounts {
                unconfirmed_challenges_deleted: 1,
                confirmed_challenges_deleted: 2,
                completed_intents_deleted: 3,
                webhook_receipts_deleted: 4,
            })
        }
    }

    #[test]
    fn should_fail_closed_for_missing_or_invalid_cleanup_batch_size() {
        for value in [
            Err(env::VarError::NotPresent),
            Ok(String::new()),
            Ok("0".to_owned()),
            Ok("1001".to_owned()),
            Ok("invalid".to_owned()),
        ] {
            assert!(cleanup_batch_size_from_result(value).is_err());
        }
    }

    #[test]
    fn should_accept_a_valid_cleanup_batch_size() {
        assert_eq!(
            cleanup_batch_size_from_result(Ok("100".to_owned())).map(ExpiryCleanupBatchSize::get),
            Ok(100),
        );
    }

    #[tokio::test]
    async fn should_invoke_each_cleanup_once_per_lambda_invocation() {
        let cleanup = RecordingCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let consent = RecordingConsentCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let geography = RecordingGeographyCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let batch_size = ExpiryCleanupBatchSize::try_from(100).unwrap();

        run_cleanup(&cleanup, &consent, &geography, batch_size)
            .await
            .unwrap();

        assert_eq!(cleanup.calls.load(Ordering::Relaxed), 1);
        assert_eq!(consent.calls.load(Ordering::Relaxed), 1);
        assert_eq!(geography.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn should_record_each_failed_cleanup_step_with_only_safe_cause_facts() {
        let cleanup = RecordingCleanup {
            calls: AtomicUsize::new(0),
            fail: true,
        };
        let consent = RecordingConsentCleanup {
            calls: AtomicUsize::new(0),
            fail: true,
        };
        let geography = RecordingGeographyCleanup {
            calls: AtomicUsize::new(0),
            fail: true,
        };
        let batch_size = ExpiryCleanupBatchSize::try_from(100).unwrap();
        let (logs, guard) = CapturedLogs::start();

        let result = run_cleanup(&cleanup, &consent, &geography, batch_size).await;
        drop(guard);

        assert!(result.is_err());
        assert_eq!(
            json!({"stage": "expired_credentials", "dependency": "postgres",
                "error_category": "protocol"}),
            failure_diagnostic(&logs, "backend_cleanup.expired_credentials.completed")
        );
        assert_eq!(
            json!({"stage": "webhook_receipts", "dependency": "postgres",
                "error_category": "tls"}),
            failure_diagnostic(&logs, "backend_cleanup.consent_workflow.completed")
        );
        assert_eq!(
            json!({"stage": "resolution_maintenance", "dependency": "postgres",
                "error_category": "pool_timed_out"}),
            failure_diagnostic(&logs, "backend_cleanup.geographic_resolution.completed")
        );
        assert!(!logs.text().contains("SENTINEL"));
    }

    #[tokio::test]
    async fn consent_failure_fails_invocation_after_existing_cleanup() {
        let cleanup = RecordingCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let consent = RecordingConsentCleanup {
            calls: AtomicUsize::new(0),
            fail: true,
        };
        let geography = RecordingGeographyCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let batch_size = ExpiryCleanupBatchSize::try_from(100).unwrap();

        assert!(
            run_cleanup(&cleanup, &consent, &geography, batch_size)
                .await
                .is_err()
        );
        assert_eq!(cleanup.calls.load(Ordering::Relaxed), 1);
        assert_eq!(consent.calls.load(Ordering::Relaxed), 1);
        assert_eq!(geography.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn existing_cleanup_failure_still_invokes_consent_cleanup_and_fails() {
        let cleanup = RecordingCleanup {
            calls: AtomicUsize::new(0),
            fail: true,
        };
        let consent = RecordingConsentCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let geography = RecordingGeographyCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let batch_size = ExpiryCleanupBatchSize::try_from(100).unwrap();

        assert!(
            run_cleanup(&cleanup, &consent, &geography, batch_size)
                .await
                .is_err()
        );
        assert_eq!(cleanup.calls.load(Ordering::Relaxed), 1);
        assert_eq!(consent.calls.load(Ordering::Relaxed), 1);
        assert_eq!(geography.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn geography_failure_fails_invocation_after_other_cleanup_and_caps_batch_size() {
        let cleanup = RecordingCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let consent = RecordingConsentCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let geography = RecordingGeographyCleanup {
            calls: AtomicUsize::new(0),
            fail: true,
        };
        let batch_size = ExpiryCleanupBatchSize::try_from(1000).unwrap();
        assert!(
            run_cleanup(&cleanup, &consent, &geography, batch_size)
                .await
                .is_err()
        );
        assert_eq!(cleanup.calls.load(Ordering::Relaxed), 1);
        assert_eq!(consent.calls.load(Ordering::Relaxed), 1);
        assert_eq!(geography.calls.load(Ordering::Relaxed), 1);
    }
}
