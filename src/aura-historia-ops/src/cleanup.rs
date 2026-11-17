//! Hourly expired-data cleanup and recovery maintenance. Every step runs, in a fixed order, even
//! when an earlier one failed; each failure is logged once with safe diagnostic facts and the
//! command fails after the last step, naming every failed step.

use crate::OpsError;
use application::operation_context::{OperationContext, Principal};
use geo_postgres::SqlxResolutionRepositoryFactory;
use geo_service::resolution::{MaintainResolutionsHandler, MaintainResolutionsUseCase};
use notification_postgres::{
    SqlxNotificationDeliveryIntentRepositoryFactory, SqlxNotificationRepositoryFactory,
    SqlxSavedSearchDigestStoreFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
    saved_search_digest::{MaintainSavedSearchDigestsHandler, MaintainSavedSearchDigestsUseCase},
};
use oauth_postgres::SqlxExpiredCredentialCleanup;
use oauth_service::ports::ExpiredCredentialCleanupError;
use oauth_service::use_cases::{
    CleanupExpiredCredentialsAndProviderReceiptsHandler,
    CleanupExpiredCredentialsAndProviderReceiptsUseCase, ExpiryCleanupBatchSize,
    ExpiryCleanupBatchSizeError,
};
use platform_postgres::SqlxUnitOfWork;
use platform_process::{Dependency, FailureDiagnostic};
use product_listing_postgres::geography::SqlxGeographyProjectionStoreFactory;
use product_listing_postgres::{
    SqlxProductListingContentAssessmentSnapshotReaderFactory,
    SqlxProductListingSearchFilterMatchSourceReaderFactory,
    SqlxPublicCatalogueOverviewRefresherFactory,
};
use product_listing_service::geography::{
    ProjectListingGeographyHandler, ProjectListingGeographyUseCase,
};
use product_listing_service::use_cases::{
    RefreshPublicCatalogueOverviewError, RefreshPublicCatalogueOverviewHandler,
    RefreshPublicCatalogueOverviewUseCase,
};
use search_filter_postgres::{
    SqlxPendingSavedSearchNotificationReader, SqlxSearchFilterMatchNotificationSourceReaderFactory,
    SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
};
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationHandler, RecoverSavedSearchNotificationsHandler,
    RecoverSavedSearchNotificationsUseCase,
};
use serde_json::{Map, Value, json};
use std::env;
use std::sync::Arc;
use std::time::Instant;
use user_postgres::{
    SqlxConsentWorkflowCleanup, SqlxNewsletterConfirmationChallengesRepository,
    SqlxUserTierEntitlementsFactory,
};
use user_service::ports::SystemNewsletterConfirmationClock;
use user_service::use_cases::{
    CleanupConsentWorkflowError, CleanupConsentWorkflowHandler, CleanupConsentWorkflowUseCase,
};

const COMMAND: &str = "backend-cleanup";
pub(crate) const EXPIRY_CLEANUP_BATCH_SIZE_ENV: &str = "EXPIRY_CLEANUP_BATCH_SIZE";
/// The geocoding provider the composition wires behind resolution and geography maintenance.
const GEOCODING_PROVIDER: Option<Dependency> = Some(Dependency::AmazonLocation);
/// Listings one run projects through unscoped geography recovery.
const LISTING_GEOGRAPHY_RECOVERY_LIMIT: u16 = 100;

pub(crate) async fn run() -> Result<Value, OpsError> {
    let batch_size =
        cleanup_batch_size_from_env().map_err(|error| OpsError::Config(error.into()))?;
    // The pool's statement timeout bounds every step; the catalogue overview refresh raises it
    // for its own transaction only (`set_config(.., true)`), so the full aggregate is not cut
    // off at the pool's bound.
    let pool = crate::pool().await?;

    let credentials = CleanupExpiredCredentialsAndProviderReceiptsHandler::new(
        SqlxExpiredCredentialCleanup::new(pool.clone()),
    );
    let consent = CleanupConsentWorkflowHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxNewsletterConfirmationChallengesRepository::new(),
        SqlxConsentWorkflowCleanup::new(),
        SystemNewsletterConfirmationClock,
    );
    let resolutions = MaintainResolutionsHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxResolutionRepositoryFactory,
    );
    let geography = ProjectListingGeographyHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxGeographyProjectionStoreFactory,
        SqlxResolutionRepositoryFactory,
    );
    let notifications = RecoverSavedSearchNotificationsHandler::new(
        SqlxPendingSavedSearchNotificationReader::new(pool.clone()),
        Arc::new(GenerateSearchFilterMatchNotificationHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxSearchFilterMatchNotificationSourceReaderFactory,
            SqlxProductListingSearchFilterMatchSourceReaderFactory,
            SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
            SqlxUserTierEntitlementsFactory::new(),
            SqlxProductListingContentAssessmentSnapshotReaderFactory,
            NotificationCreationCoordinatorFactory::new(
                SqlxNotificationRepositoryFactory,
                InitialExternalDeliveryPlanReaderFactory,
                SqlxNotificationDeliveryIntentRepositoryFactory,
            ),
            SqlxSavedSearchDigestStoreFactory,
        )),
    );
    let digests = MaintainSavedSearchDigestsHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxSavedSearchDigestStoreFactory,
    );
    let overview = RefreshPublicCatalogueOverviewHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxPublicCatalogueOverviewRefresherFactory::new(),
    );

    let mut steps = Vec::from(run_cleanup(&credentials, &consent, &resolutions, batch_size).await);
    steps.push(run_listing_geography_recovery(&geography).await);
    steps.push(
        run_saved_search_notification_recovery(&notifications, batch_size.get() as usize).await,
    );
    steps.push(run_saved_search_digest_maintenance(&digests, batch_size.get() as usize).await);
    // Last: when due, the full aggregate must not delay bounded maintenance work.
    steps.push(run_public_catalogue_overview_refresh(&overview).await);
    report(steps)
}

/// The outcome of one maintenance step: its report entry, and whether it failed.
#[derive(Debug)]
pub(crate) struct Step {
    name: &'static str,
    succeeded: bool,
    report: Value,
}

impl Step {
    fn new(name: &'static str, succeeded: bool, report: Value) -> Self {
        Self {
            name,
            succeeded,
            report,
        }
    }

    fn failed(name: &'static str) -> Self {
        Self::new(name, false, json!({"outcome": "failure"}))
    }
}

/// The names of the failed steps. Their causes were logged once by the step itself.
#[derive(Debug, thiserror::Error)]
#[error("maintenance steps failed: {}", .0.join(", "))]
pub(crate) struct StepsFailed(Vec<&'static str>);

/// One entry per step, in run order; fails naming every failed step once all have run.
fn report(steps: Vec<Step>) -> Result<Value, OpsError> {
    let failed: Vec<_> = steps
        .iter()
        .filter(|step| !step.succeeded)
        .map(|step| step.name)
        .collect();
    if !failed.is_empty() {
        return Err(OpsError::failed(COMMAND, StepsFailed(failed)));
    }
    let steps: Map<String, Value> = steps
        .into_iter()
        .map(|step| (step.name.to_owned(), step.report))
        .collect();
    Ok(json!({ "steps": steps }))
}

/// Runs last in the hourly maintenance run; the use case recalculates only once the stored
/// calculation is due, so most runs skip the full aggregate.
pub(crate) async fn run_public_catalogue_overview_refresh(
    refresh: &dyn RefreshPublicCatalogueOverviewUseCase,
) -> Step {
    const NAME: &str = "publicCatalogueOverviewRefresh";
    let started_at = Instant::now();
    match refresh.execute().await {
        Ok(None) => {
            tracing::info!(
                event = "backend_cleanup.public_catalogue_overview.refreshed",
                outcome = "current",
                duration_ms = started_at.elapsed().as_millis(),
            );
            Step::new(NAME, true, json!({"outcome": "current"}))
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
            Step::new(
                NAME,
                true,
                json!({
                    "outcome": "success",
                    "stored": refresh.stored,
                    "liveObjects": refresh.overview.live_objects,
                    "sourcesIndexed": refresh.overview.sources_indexed,
                }),
            )
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
            Step::failed(NAME)
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
pub(crate) async fn run_listing_geography_recovery(
    recovery: &dyn ProjectListingGeographyUseCase,
) -> Step {
    const NAME: &str = "listingGeographyRecovery";
    match recovery
        .execute(None, LISTING_GEOGRAPHY_RECOVERY_LIMIT)
        .await
    {
        Ok(projected) => Step::new(
            NAME,
            true,
            json!({"outcome": "success", "projected": projected}),
        ),
        Err(error) => {
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
            Step::failed(NAME)
        }
    }
}

/// Generates saved-search notifications whose wake-up was lost. Each failed selection is
/// deferred by the use case and recorded here once; any failure fails the step.
pub(crate) async fn run_saved_search_notification_recovery(
    recovery: &dyn RecoverSavedSearchNotificationsUseCase,
    limit: usize,
) -> Step {
    const NAME: &str = "savedSearchNotificationRecovery";
    let started_at = Instant::now();
    let report = match recovery.execute(limit).await {
        Ok(report) => report,
        Err(error) => {
            let failure = FailureDiagnostic::postgres("saved_search_notification_read", &error);
            tracing::warn!(
                event = "backend_cleanup.saved_search_notification_recovery.completed",
                outcome = "failure",
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                sqlstate = failure.sqlstate(),
                duration_ms = started_at.elapsed().as_millis(),
            );
            return Step::failed(NAME);
        }
    };
    for failed in &report.failures {
        let failure = FailureDiagnostic::from_kind(
            failed.error.failure_stage(),
            failed.error.failure_kind(),
            None,
            &failed.error,
        );
        tracing::warn!(
            event = "backend_cleanup.saved_search_notification.recovery_failed",
            user_id = %failed.user_id,
            search_filter_id = %failed.search_filter_id,
            stage = failure.stage(),
            dependency = failure.dependency(),
            error_category = failure.category(),
            sqlstate = failure.sqlstate(),
        );
    }
    if let Some(error) = &report.defer_failure {
        let failure = FailureDiagnostic::postgres("saved_search_notification_defer", error);
        tracing::warn!(
            event = "backend_cleanup.saved_search_notification_recovery.completed",
            outcome = "failure",
            recovered = report.recovered,
            failed = report.failures.len(),
            stage = failure.stage(),
            dependency = failure.dependency(),
            error_category = failure.category(),
            sqlstate = failure.sqlstate(),
            duration_ms = started_at.elapsed().as_millis(),
        );
        return Step::new(
            NAME,
            false,
            json!({"outcome": "failure", "recovered": report.recovered,
                "failed": report.failures.len()}),
        );
    }
    let outcome = if report.failures.is_empty() {
        "success"
    } else {
        "incomplete"
    };
    tracing::info!(
        event = "backend_cleanup.saved_search_notification_recovery.completed",
        outcome,
        recovered = report.recovered,
        failed = report.failures.len(),
        duration_ms = started_at.elapsed().as_millis(),
    );
    Step::new(
        NAME,
        report.failures.is_empty(),
        json!({"outcome": outcome, "recovered": report.recovered,
            "failed": report.failures.len()}),
    )
}

/// Finalizes ready saved-search digest groups that no completed selection finalized.
pub(crate) async fn run_saved_search_digest_maintenance(
    maintenance: &dyn MaintainSavedSearchDigestsUseCase,
    limit: usize,
) -> Step {
    const NAME: &str = "savedSearchDigestMaintenance";
    let started_at = Instant::now();
    match maintenance.execute(limit).await {
        Ok(report) => Step::new(
            NAME,
            true,
            json!({
                "outcome": "success",
                "groupsFinalized": report.groups_finalized,
                "digestsCreated": report.digests_created,
                "matchesEnrolled": report.matches_enrolled,
            }),
        ),
        Err(error) => {
            let failure = FailureDiagnostic::postgres("saved_search_digest_maintenance", &error);
            tracing::warn!(
                event = "backend_cleanup.saved_search_digest_maintenance.completed",
                outcome = "failure",
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                sqlstate = failure.sqlstate(),
                duration_ms = started_at.elapsed().as_millis(),
            );
            Step::failed(NAME)
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum CleanupConfigError {
    #[error("missing required cleanup configuration {name}")]
    Missing { name: &'static str },
    #[error("invalid cleanup configuration {name}")]
    Invalid { name: &'static str },
}

pub(crate) fn cleanup_batch_size_from_env() -> Result<ExpiryCleanupBatchSize, CleanupConfigError> {
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

/// Expired credentials and provider receipts, the consent workflow, and geographic resolution
/// maintenance, each run even when an earlier one failed.
pub(crate) async fn run_cleanup(
    cleanup: &(dyn CleanupExpiredCredentialsAndProviderReceiptsUseCase + Send + Sync),
    consent_cleanup: &(dyn CleanupConsentWorkflowUseCase + Send + Sync),
    geography_cleanup: &dyn MaintainResolutionsUseCase,
    batch_size: ExpiryCleanupBatchSize,
) -> [Step; 3] {
    const CREDENTIALS: &str = "expiredCredentials";
    const CONSENT: &str = "consentWorkflow";
    const RESOLUTION: &str = "geographicResolution";
    let started_at = Instant::now();
    let credential_step = match cleanup.execute(batch_size).await {
        Ok(counts) => {
            tracing::info!(
                event = "backend_cleanup.expired_credentials.completed",
                outcome = "success",
                batch_size = batch_size.get(),
                access_tokens_deleted = counts.access_tokens_deleted,
                authorization_codes_deleted = counts.authorization_codes_deleted,
                third_party_exchange_codes_deleted = counts.third_party_exchange_codes_deleted,
                provider_receipts_deleted = counts.provider_receipts_deleted,
                refresh_tokens_deleted = counts.refresh_tokens_deleted,
                refresh_token_families_deleted = counts.refresh_token_families_deleted,
                oauth_connections_deleted = counts.oauth_connections_deleted,
                duration_ms = started_at.elapsed().as_millis(),
            );
            Step::new(
                CREDENTIALS,
                true,
                json!({
                    "outcome": "success",
                    "accessTokensDeleted": counts.access_tokens_deleted,
                    "authorizationCodesDeleted": counts.authorization_codes_deleted,
                    "thirdPartyExchangeCodesDeleted": counts.third_party_exchange_codes_deleted,
                    "providerReceiptsDeleted": counts.provider_receipts_deleted,
                    "refreshTokensDeleted": counts.refresh_tokens_deleted,
                    "refreshTokenFamiliesDeleted": counts.refresh_token_families_deleted,
                    "oauthConnectionsDeleted": counts.oauth_connections_deleted,
                }),
            )
        }
        Err(error) => {
            let failure = expired_credential_failure(&error);
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
            Step::failed(CREDENTIALS)
        }
    };
    let consent_step = match consent_cleanup.execute(batch_size.get() as u16).await {
        Ok(counts) => {
            tracing::info!(
                event = "backend_cleanup.consent_workflow.completed",
                outcome = "success",
                batch_size = batch_size.get(),
                unconfirmed_challenges_deleted = counts.unconfirmed_challenges_deleted,
                confirmed_challenges_deleted = counts.confirmed_challenges_deleted,
                completed_intents_deleted = counts.completed_intents_deleted,
                webhook_receipts_deleted = counts.webhook_receipts_deleted,
                duration_ms = started_at.elapsed().as_millis(),
            );
            Step::new(
                CONSENT,
                true,
                json!({
                    "outcome": "success",
                    "unconfirmedChallengesDeleted": counts.unconfirmed_challenges_deleted,
                    "confirmedChallengesDeleted": counts.confirmed_challenges_deleted,
                    "completedIntentsDeleted": counts.completed_intents_deleted,
                    "webhookReceiptsDeleted": counts.webhook_receipts_deleted,
                }),
            )
        }
        Err(error) => {
            let failure = consent_cleanup_failure(&error);
            tracing::warn!(
                event = "backend_cleanup.consent_workflow.completed",
                outcome = "failure",
                batch_size = batch_size.get(),
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                sqlstate = failure.sqlstate(),
                duration_ms = started_at.elapsed().as_millis(),
            );
            Step::failed(CONSENT)
        }
    };
    let context = OperationContext {
        principal: Principal::Service("backend-cleanup-worker".into()),
        request_id: "backend-cleanup".into(),
        correlation_id: "backend-cleanup".into(),
    };
    let geography_limit = batch_size.get().min(100) as u32;
    let resolution_step = match geography_cleanup.execute(&context, geography_limit).await {
        Ok(counts) => {
            tracing::info!(
                event = "backend_cleanup.geographic_resolution.completed",
                outcome = "success",
                batch_size = geography_limit,
                expired = counts.expired,
                requeued = counts.requeued,
                duration_ms = started_at.elapsed().as_millis(),
            );
            Step::new(
                RESOLUTION,
                true,
                json!({"outcome": "success", "expired": counts.expired,
                    "requeued": counts.requeued}),
            )
        }
        Err(error) => {
            let failure = FailureDiagnostic::from_kind(
                "resolution_maintenance",
                error.failure_kind(),
                GEOCODING_PROVIDER,
                &error,
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
            );
            Step::failed(RESOLUTION)
        }
    };
    [credential_step, consent_step, resolution_step]
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
    use notification_service::saved_search_digest::{
        SavedSearchDigestError, SavedSearchDigestMaintenanceReport,
    };
    use oauth_service::use_cases::CleanupCounts;
    use product_listing_service::geography::{GeographyError, GeographyWorkId};
    use product_listing_service::use_cases::{
        PublicCatalogueOverview, PublicCatalogueOverviewRefresh,
    };
    use search_filter_core::user_search_filter_id::UserSearchFilterId;
    use search_filter_service::use_cases::{
        GenerateSearchFilterMatchNotificationError, SavedSearchNotificationRecoveryError,
        SavedSearchNotificationRecoveryFailure, SavedSearchNotificationRecoveryReport,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_api::CapturedLogs;
    use user_core::user_id::UserId;
    use user_service::use_cases::{ConsentWorkflowCleanupCounts, ConsentWorkflowCleanupTarget};

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

    fn failed(steps: &[Step]) -> bool {
        steps.iter().any(|step| !step.succeeded)
    }

    #[test]
    fn should_report_every_step_and_fail_naming_each_failed_step() {
        let report = report(vec![
            Step::new("first", true, json!({"outcome": "success"})),
            Step::new("second", true, json!({"outcome": "current"})),
        ])
        .expect("every step succeeded");
        assert_eq!(
            json!({"steps": {"first": {"outcome": "success"}, "second": {"outcome": "current"}}}),
            report
        );

        let Err(OpsError::Failed { command, source }) = super::report(vec![
            Step::failed("first"),
            Step::new("second", true, json!({})),
            Step::failed("third"),
        ]) else {
            panic!("a failed step fails the command");
        };
        assert_eq!(COMMAND, command);
        assert_eq!("maintenance steps failed: first, third", source.to_string());
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
                    sources_indexed_by_type:
                        product_listing_service::use_cases::SourcesIndexedByType {
                            dealers: 1,
                            auction_houses: 0,
                            marketplaces: 0,
                            auction_platforms: 0,
                        },
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
    async fn should_fail_the_step_only_for_a_failed_overview_refresh() {
        for (due, fail, outcome) in [
            (true, false, "success"),
            (false, false, "current"),
            (true, true, "failure"),
        ] {
            let refresh = RecordingOverviewRefresh {
                calls: AtomicUsize::new(0),
                due,
                fail,
            };

            let step = run_public_catalogue_overview_refresh(&refresh).await;

            assert_eq!(fail, !step.succeeded);
            assert_eq!(Some(outcome), step.report["outcome"].as_str());
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

        let step = run_public_catalogue_overview_refresh(&refresh).await;
        drop(guard);

        assert!(!step.succeeded);
        assert_eq!(
            json!({"stage": "overview_calculation", "dependency": "postgres",
                "error_category": "io"}),
            failure_diagnostic(&logs, "backend_cleanup.public_catalogue_overview.refreshed")
        );
        assert!(!logs.text().contains("SENTINEL"));
        assert!(!step.report.to_string().contains("SENTINEL"));
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

        let step = run_listing_geography_recovery(&FailingGeographyRecovery).await;
        drop(guard);

        assert!(!step.succeeded);
        assert_eq!(
            json!({"stage": "geography_transaction", "dependency": "postgres",
                "error_category": "io"}),
            failure_diagnostic(&logs, "backend_cleanup.listing_geography.failed")
        );
        assert!(!logs.text().contains("SENTINEL"));
    }

    struct PartialNotificationRecovery;

    #[async_trait::async_trait]
    impl RecoverSavedSearchNotificationsUseCase for PartialNotificationRecovery {
        async fn execute(
            &self,
            _: usize,
        ) -> Result<SavedSearchNotificationRecoveryReport, SavedSearchNotificationRecoveryError>
        {
            Ok(SavedSearchNotificationRecoveryReport {
                recovered: 1,
                failures: vec![SavedSearchNotificationRecoveryFailure {
                    user_id: UserId::new(),
                    search_filter_id: UserSearchFilterId::new(),
                    error:
                        GenerateSearchFilterMatchNotificationError::MonthlyMatchQuotaReadFailed {
                            source: Box::new(sqlx::Error::PoolTimedOut),
                        },
                }],
                defer_failure: None,
            })
        }
    }

    struct FailingNotificationRecovery;

    #[async_trait::async_trait]
    impl RecoverSavedSearchNotificationsUseCase for FailingNotificationRecovery {
        async fn execute(
            &self,
            _: usize,
        ) -> Result<SavedSearchNotificationRecoveryReport, SavedSearchNotificationRecoveryError>
        {
            Err(SavedSearchNotificationRecoveryError {
                source: Box::new(sqlx::Error::Io(std::io::Error::other(SENTINEL))),
            })
        }
    }

    struct FailingDigestMaintenance;

    #[async_trait::async_trait]
    impl MaintainSavedSearchDigestsUseCase for FailingDigestMaintenance {
        async fn execute(
            &self,
            _: usize,
        ) -> Result<SavedSearchDigestMaintenanceReport, SavedSearchDigestError> {
            Err(SavedSearchDigestError {
                source: Box::new(TransactionError::CommitFailed(Box::new(sqlx::Error::Io(
                    std::io::Error::other(SENTINEL),
                )))),
            })
        }
    }

    #[tokio::test]
    async fn should_record_each_failed_saved_search_selection_and_fail_the_step() {
        let (logs, guard) = CapturedLogs::start();

        let step = run_saved_search_notification_recovery(&PartialNotificationRecovery, 10).await;
        drop(guard);

        assert!(!step.succeeded);
        assert_eq!(
            json!({"outcome": "incomplete", "recovered": 1, "failed": 1}),
            step.report
        );
        assert_eq!(
            json!({"stage": "monthly_quota_read", "dependency": "postgres",
                "error_category": "pool_timed_out"}),
            failure_diagnostic(
                &logs,
                "backend_cleanup.saved_search_notification.recovery_failed"
            )
        );
        let completed = logs.events_where(
            "event",
            "backend_cleanup.saved_search_notification_recovery.completed",
        );
        assert_eq!(Some("incomplete"), completed[0]["outcome"].as_str());
        assert_eq!(Some(1), completed[0]["failed"].as_u64());
    }

    #[tokio::test]
    async fn should_record_failed_saved_search_recovery_steps_without_their_cause() {
        let (logs, guard) = CapturedLogs::start();

        let notifications =
            run_saved_search_notification_recovery(&FailingNotificationRecovery, 10).await;
        let digests = run_saved_search_digest_maintenance(&FailingDigestMaintenance, 10).await;
        drop(guard);

        assert!(!notifications.succeeded);
        assert!(!digests.succeeded);
        assert_eq!(
            json!({"stage": "saved_search_notification_read", "dependency": "postgres",
                "error_category": "io"}),
            failure_diagnostic(
                &logs,
                "backend_cleanup.saved_search_notification_recovery.completed"
            )
        );
        assert_eq!(
            json!({"stage": "saved_search_digest_maintenance", "dependency": "postgres",
                "error_category": "io"}),
            failure_diagnostic(
                &logs,
                "backend_cleanup.saved_search_digest_maintenance.completed"
            )
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
                refresh_tokens_deleted: 5,
                refresh_token_families_deleted: 6,
                oauth_connections_deleted: 7,
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

    fn recording(
        fail: [bool; 3],
    ) -> (
        RecordingCleanup,
        RecordingConsentCleanup,
        RecordingGeographyCleanup,
    ) {
        (
            RecordingCleanup {
                calls: AtomicUsize::new(0),
                fail: fail[0],
            },
            RecordingConsentCleanup {
                calls: AtomicUsize::new(0),
                fail: fail[1],
            },
            RecordingGeographyCleanup {
                calls: AtomicUsize::new(0),
                fail: fail[2],
            },
        )
    }

    fn assert_each_called_once(
        (cleanup, consent, geography): &(
            RecordingCleanup,
            RecordingConsentCleanup,
            RecordingGeographyCleanup,
        ),
    ) {
        assert_eq!(cleanup.calls.load(Ordering::Relaxed), 1);
        assert_eq!(consent.calls.load(Ordering::Relaxed), 1);
        assert_eq!(geography.calls.load(Ordering::Relaxed), 1);
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
    async fn should_invoke_each_cleanup_once_per_run_and_report_its_counts() {
        let recording = recording([false; 3]);
        let batch_size = ExpiryCleanupBatchSize::try_from(100).unwrap();

        let steps = run_cleanup(&recording.0, &recording.1, &recording.2, batch_size).await;

        assert!(!failed(&steps));
        assert_each_called_once(&recording);
        assert_eq!(
            [
                "expiredCredentials",
                "consentWorkflow",
                "geographicResolution"
            ],
            steps.each_ref().map(|step| step.name)
        );
        assert_eq!(Some(7), steps[0].report["oauthConnectionsDeleted"].as_u64());
        assert_eq!(Some(4), steps[1].report["webhookReceiptsDeleted"].as_u64());
        assert_eq!(Some(2), steps[2].report["requeued"].as_u64());
    }

    #[tokio::test]
    async fn should_record_each_failed_cleanup_step_with_only_safe_cause_facts() {
        let recording = recording([true; 3]);
        let batch_size = ExpiryCleanupBatchSize::try_from(100).unwrap();
        let (logs, guard) = CapturedLogs::start();

        let steps = run_cleanup(&recording.0, &recording.1, &recording.2, batch_size).await;
        drop(guard);

        assert!(steps.iter().all(|step| !step.succeeded));
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
    async fn should_run_every_cleanup_step_when_any_one_fails() {
        // Geography also caps a batch size above 100.
        for (fail, batch_size) in [
            ([false, true, false], 100),
            ([true, false, false], 100),
            ([false, false, true], 1000),
        ] {
            let recording = recording(fail);
            let batch_size = ExpiryCleanupBatchSize::try_from(batch_size).unwrap();

            let steps = run_cleanup(&recording.0, &recording.1, &recording.2, batch_size).await;

            assert!(failed(&steps), "{fail:?}");
            assert_eq!(fail, steps.each_ref().map(|step| !step.succeeded));
            assert_each_called_once(&recording);
        }
    }
}
