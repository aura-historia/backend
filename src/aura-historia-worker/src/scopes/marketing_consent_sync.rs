//! `marketing-consent-sync`: reconciles one marketing consent intent with Loops in the
//! recipient's FIFO group.

use crate::composition::CompositionError;
use async_trait::async_trait;
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use platform_postgres_queue::Disposition;
use platform_process::{
    Dependency, FailureDiagnostic, INVALID_PERSISTED_STATE,
    config::{optional, required},
};
use platform_worker::{Job, JobHandler, error_class};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::{info, warn};
use user_loops::{LoopsMarketingEmailConsentWriter, LoopsNewsletterConfig};
use user_postgres::{SqlxMarketingConsentIntentRepository, SqlxMarketingConsentIntentWorker};
use user_service::use_cases::{
    SyncMarketingConsentIntentError, SyncMarketingConsentIntentHandler,
    SyncMarketingConsentIntentResult, SyncMarketingConsentIntentUseCase,
};

const SCOPE: &str = "marketing-consent-sync";
const DEFAULT_LOOPS_API_BASE_URL: &str = "https://app.loops.so/api";

pub fn compose_marketing_consent_sync_use_case(
    pool: PgPool,
    loops: LoopsNewsletterConfig,
) -> Result<Arc<dyn SyncMarketingConsentIntentUseCase>, CompositionError> {
    let provider = LoopsMarketingEmailConsentWriter::new(loops)
        .map_err(|error| CompositionError::new(SCOPE, error))?;
    let repository = SqlxMarketingConsentIntentRepository::new();
    Ok(Arc::new(SyncMarketingConsentIntentHandler::new(
        platform_postgres::SqlxUnitOfWork::new(pool),
        SqlxMarketingConsentIntentWorker::new(),
        repository,
        provider,
    )))
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    let use_case = compose_marketing_consent_sync_use_case(pool.clone(), loops_config()?)?;
    Ok(Arc::new(MarketingConsentSyncJobs::new(use_case)))
}

fn loops_config() -> Result<LoopsNewsletterConfig, CompositionError> {
    let setting = |error| CompositionError::new(SCOPE, error);
    let api_key = required("LOOPS_API_KEY").map_err(setting)?;
    let list_id = required("LOOPS_NEWSLETTER_LIST_ID").map_err(setting)?;
    let base_url =
        optional("LOOPS_API_BASE_URL", DEFAULT_LOOPS_API_BASE_URL.to_owned()).map_err(setting)?;
    LoopsNewsletterConfig::new(api_key, list_id, base_url)
        .map_err(|error| CompositionError::new(SCOPE, error))
}

pub struct MarketingConsentSyncJobs {
    use_case: Arc<dyn SyncMarketingConsentIntentUseCase>,
}

impl MarketingConsentSyncJobs {
    pub fn new(use_case: Arc<dyn SyncMarketingConsentIntentUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for MarketingConsentSyncJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let outcome =
            process_marketing_consent_sync_job(job.payload(), self.use_case.as_ref()).await;
        let disposition = outcome.queue_disposition();
        match disposition {
            Disposition::Complete => {}
            Disposition::Park(_) => warn!(
                job_id = %job.id(),
                attempt = job.attempt(),
                outcome = outcome.category(),
                "Marketing consent sync job cannot complete as delivered"
            ),
            Disposition::Retry(_) => warn!(
                job_id = %job.id(),
                attempt = job.attempt(),
                outcome = outcome.category(),
                "Marketing consent sync job retained for retry"
            ),
        }
        disposition
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketingConsentSyncJobDisposition {
    Applied,
    Blocked,
    Superseded,
    AlreadyTerminal,
    Deferred,
    Missing,
    Retry,
    Poison,
}

impl MarketingConsentSyncJobDisposition {
    /// An undecodable job and a missing intent can never complete: a job never recreates its
    /// intent, so both are parked and later intents of the recipient continue.
    pub fn queue_disposition(self) -> Disposition {
        match self {
            Self::Applied | Self::Blocked | Self::Superseded | Self::AlreadyTerminal => {
                Disposition::Complete
            }
            Self::Missing | Self::Poison => Disposition::Park(error_class(self.category())),
            Self::Deferred | Self::Retry => Disposition::Retry(error_class(self.category())),
        }
    }

    const fn category(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Blocked => "blocked",
            Self::Superseded => "superseded",
            Self::AlreadyTerminal => "already_terminal",
            Self::Deferred => "active_lease_deferred",
            Self::Missing => "intent_missing",
            Self::Retry => "sync_unconfirmed",
            Self::Poison => "invalid_wire_job",
        }
    }
}

pub async fn process_marketing_consent_sync_job(
    body: &str,
    use_case: &(dyn SyncMarketingConsentIntentUseCase + Send + Sync),
) -> MarketingConsentSyncJobDisposition {
    let job = match decode(body, WorkerScope::MarketingConsentSync) {
        Ok(job) => job,
        Err(_) => return MarketingConsentSyncJobDisposition::Poison,
    };
    let intent_id = match intent_id_from_job(job) {
        Ok(intent_id) => intent_id,
        Err(_) => return MarketingConsentSyncJobDisposition::Poison,
    };
    match use_case.execute(intent_id).await {
        Ok(result) => {
            let disposition = match result {
                SyncMarketingConsentIntentResult::Applied => {
                    MarketingConsentSyncJobDisposition::Applied
                }
                SyncMarketingConsentIntentResult::Blocked => {
                    MarketingConsentSyncJobDisposition::Blocked
                }
                SyncMarketingConsentIntentResult::Superseded => {
                    MarketingConsentSyncJobDisposition::Superseded
                }
                SyncMarketingConsentIntentResult::AlreadyTerminal => {
                    MarketingConsentSyncJobDisposition::AlreadyTerminal
                }
                SyncMarketingConsentIntentResult::Deferred => {
                    MarketingConsentSyncJobDisposition::Deferred
                }
                SyncMarketingConsentIntentResult::Retryable => {
                    MarketingConsentSyncJobDisposition::Retry
                }
                SyncMarketingConsentIntentResult::Missing => {
                    MarketingConsentSyncJobDisposition::Missing
                }
            };
            info!(
                intent_id = %intent_id,
                outcome = disposition.category(),
                "Marketing consent sync intent processed"
            );
            disposition
        }
        Err(error) => {
            let failure = sync_failure(&error);
            warn!(
                intent_id = %intent_id,
                outcome = "sync_unconfirmed",
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                http_status = failure.http_status(),
                sqlstate = failure.sqlstate(),
                "Marketing consent sync intent remains unfinished"
            );
            MarketingConsentSyncJobDisposition::Retry
        }
    }
}

/// Keeps the failed stage and safe cause facts; the error chain itself is never logged.
fn sync_failure(error: &SyncMarketingConsentIntentError) -> FailureDiagnostic {
    use SyncMarketingConsentIntentError as E;
    match error {
        E::PersistenceFailed(_) => FailureDiagnostic::postgres("intent_persistence", error),
        E::ConcurrentChange => {
            FailureDiagnostic::rejected("intent_persistence", "concurrency_conflict")
        }
        E::ProviderReadFailed(provider) => FailureDiagnostic::new(
            "provider_read",
            Dependency::Loops,
            provider.diagnostic_category(),
            provider.http_status(),
        ),
        E::RevokeUnconfirmed => FailureDiagnostic::new(
            "revoke_reconciliation",
            Dependency::Loops,
            "revoke_unconfirmed",
            None,
        ),
        E::Transaction(_) => FailureDiagnostic::postgres("intent_transaction", error),
        E::ReconciliationExhausted { .. } => FailureDiagnostic::postgres("reconciliation", error),
        E::InvalidPersistedState => FailureDiagnostic::new(
            "intent_read",
            Dependency::Postgres,
            INVALID_PERSISTED_STATE,
            None,
        ),
    }
}

pub fn intent_id_from_job(
    job: DomainJob,
) -> Result<
    user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId,
    aura_historia_jobs::InvalidJob,
> {
    let DomainJobPayload::MarketingConsentSyncIntentCreated(job) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(job.marketing_consent_sync_intent_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::fifo_job;
    use std::{collections::VecDeque, sync::Mutex};
    use test_api::CapturedLogs;
    use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
    use user_service::ports::MarketingEmailConsentError;

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let (_, jobs) = handler([Err(SyncMarketingConsentIntentError::Transaction(Box::new(
            sqlx::Error::Io(std::io::Error::other(
                "SENTINEL_PAYLOAD_TOKEN_person@example.com",
            )),
        )))]);
        let (logs, guard) = CapturedLogs::start();

        let disposition = jobs.handle(&job(&valid_body())).await;
        drop(guard);

        assert_eq!(
            Disposition::Retry(error_class("sync_unconfirmed")),
            disposition
        );
        let failures = logs.events_where(
            "message",
            "Marketing consent sync intent remains unfinished",
        );
        assert_eq!(1, failures.len());
        assert_eq!(Some("intent_transaction"), failures[0]["stage"].as_str());
        assert_eq!(Some("postgres"), failures[0]["dependency"].as_str());
        assert_eq!(Some("io"), failures[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }

    #[tokio::test]
    async fn should_attribute_provider_and_persistence_failures_to_their_system() {
        let cases = [
            (
                SyncMarketingConsentIntentError::ProviderReadFailed(
                    MarketingEmailConsentError::Throttled { status: Some(429) },
                ),
                ("provider_read", "loops", "throttled", Some(429)),
            ),
            (
                SyncMarketingConsentIntentError::RevokeUnconfirmed,
                ("revoke_reconciliation", "loops", "revoke_unconfirmed", None),
            ),
            (
                SyncMarketingConsentIntentError::PersistenceFailed(Box::new(sqlx::Error::Io(
                    std::io::Error::other("SENTINEL_PAYLOAD_TOKEN_person@example.com"),
                ))),
                ("intent_persistence", "postgres", "io", None),
            ),
        ];
        for (error, (stage, dependency, category, http_status)) in cases {
            let (_, jobs) = handler([Err(error)]);
            let (logs, guard) = CapturedLogs::start();

            let disposition = jobs.handle(&job(&valid_body())).await;
            drop(guard);

            assert_eq!(
                Disposition::Retry(error_class("sync_unconfirmed")),
                disposition,
                "{stage}"
            );
            let failures = logs.events_where(
                "message",
                "Marketing consent sync intent remains unfinished",
            );
            assert_eq!(1, failures.len(), "{stage}");
            assert_eq!(Some(stage), failures[0]["stage"].as_str());
            assert_eq!(Some(dependency), failures[0]["dependency"].as_str());
            assert_eq!(Some(category), failures[0]["error_category"].as_str());
            assert_eq!(
                http_status,
                failures[0]
                    .get("http_status")
                    .and_then(|status| status.as_u64())
            );
            assert!(!logs.text().contains("SENTINEL"), "{stage}");
        }
    }

    #[tokio::test]
    async fn should_map_every_sync_result_to_its_queue_disposition() {
        let cases = [
            (
                Ok(SyncMarketingConsentIntentResult::Applied),
                Disposition::Complete,
            ),
            (
                Ok(SyncMarketingConsentIntentResult::Blocked),
                Disposition::Complete,
            ),
            (
                Ok(SyncMarketingConsentIntentResult::Superseded),
                Disposition::Complete,
            ),
            (
                Ok(SyncMarketingConsentIntentResult::AlreadyTerminal),
                Disposition::Complete,
            ),
            (
                Ok(SyncMarketingConsentIntentResult::Deferred),
                Disposition::Retry(error_class("active_lease_deferred")),
            ),
            (
                Ok(SyncMarketingConsentIntentResult::Retryable),
                Disposition::Retry(error_class("sync_unconfirmed")),
            ),
            (
                Err(SyncMarketingConsentIntentError::Transaction(
                    application::error::static_error("test transaction failure"),
                )),
                Disposition::Retry(error_class("sync_unconfirmed")),
            ),
            (
                Ok(SyncMarketingConsentIntentResult::Missing),
                Disposition::Park(error_class("intent_missing")),
            ),
        ];
        for (result, expected) in cases {
            let (use_case, jobs) = handler([result]);
            assert_eq!(expected, jobs.handle(&job(&valid_body())).await);
            assert_eq!(1, use_case.call_count());
        }
    }

    #[tokio::test]
    async fn malformed_jobs_are_parked_without_logging_bodies() {
        for body in ["sensitive body that is not JSON", ""] {
            let (use_case, jobs) = handler([]);
            let (logs, guard) = CapturedLogs::start();

            let disposition = jobs.handle(&job(body)).await;
            drop(guard);

            assert_eq!(
                Disposition::Park(error_class("invalid_wire_job")),
                disposition
            );
            assert_eq!(0, use_case.call_count());
            assert!(!logs.text().contains("sensitive body"));
        }
    }

    #[tokio::test]
    async fn should_park_invalid_jobs_and_missing_intents_when_redelivered_as_on_first_delivery() {
        // The queue releases a parked first delivery and moves a redelivery to custody; the
        // handler reports the same disposition either way.
        for attempt in [1, 2] {
            let (logs, guard) = CapturedLogs::start();
            let (_, jobs) = handler([]);
            let invalid = fifo_job("not JSON", "recipient", attempt);
            assert_eq!(
                Disposition::Park(error_class("invalid_wire_job")),
                jobs.handle(&invalid).await
            );
            let (_, jobs) = handler([Ok(SyncMarketingConsentIntentResult::Missing)]);
            assert_eq!(
                Disposition::Park(error_class("intent_missing")),
                jobs.handle(&fifo_job(&valid_body(), "recipient", attempt))
                    .await
            );
            drop(guard);

            let parked = logs.events_where(
                "message",
                "Marketing consent sync job cannot complete as delivered",
            );
            assert_eq!(2, parked.len());
            assert_eq!(
                Some(invalid.id().to_string().as_str()),
                parked[0]["job_id"].as_str()
            );
            assert_eq!(Some(u64::from(attempt)), parked[0]["attempt"].as_u64());
            assert_eq!(Some("invalid_wire_job"), parked[0]["outcome"].as_str());
            assert_eq!(Some("intent_missing"), parked[1]["outcome"].as_str());
        }
    }

    #[tokio::test]
    async fn should_keep_active_leases_and_unconfirmed_syncs_retained_on_redelivery() {
        for (result, category) in [
            (
                SyncMarketingConsentIntentResult::Deferred,
                "active_lease_deferred",
            ),
            (
                SyncMarketingConsentIntentResult::Retryable,
                "sync_unconfirmed",
            ),
        ] {
            let (logs, guard) = CapturedLogs::start();
            let (_, jobs) = handler([Ok(result)]);
            let job = fifo_job(&valid_body(), "recipient", 5);

            let disposition = jobs.handle(&job).await;
            drop(guard);

            assert_eq!(Disposition::Retry(error_class(category)), disposition);
            let retained =
                logs.events_where("message", "Marketing consent sync job retained for retry");
            assert_eq!(1, retained.len());
            assert_eq!(
                Some(job.id().to_string().as_str()),
                retained[0]["job_id"].as_str()
            );
            assert_eq!(Some(category), retained[0]["outcome"].as_str());
        }
    }

    fn valid_body() -> String {
        r#"{"schema_version":1,"scope":"marketing-consent-sync","idempotency_key":"marketing-consent:mci_01h455vb4pex5vy7enb1p677vn","ordering_key":"marketing-email:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","job_type":"MARKETING_CONSENT_SYNC_INTENT_CREATED","payload":{"marketing_consent_sync_intent_id":"mci_01h455vb4pex5vy7enb1p677vn"}}"#.to_owned()
    }

    fn job(body: &str) -> Job {
        fifo_job(body, "recipient", 1)
    }

    fn handler<const N: usize>(
        outcomes: [Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError>; N],
    ) -> (Arc<FakeUseCase>, MarketingConsentSyncJobs) {
        let use_case = Arc::new(FakeUseCase {
            outcomes: Mutex::new(outcomes.into()),
            calls: Mutex::new(Vec::new()),
        });
        let jobs = MarketingConsentSyncJobs::new(use_case.clone());
        (use_case, jobs)
    }

    struct FakeUseCase {
        outcomes: Mutex<
            VecDeque<Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError>>,
        >,
        calls: Mutex<Vec<MarketingConsentSyncIntentId>>,
    }

    impl FakeUseCase {
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl SyncMarketingConsentIntentUseCase for FakeUseCase {
        async fn execute(
            &self,
            intent_id: MarketingConsentSyncIntentId,
        ) -> Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError> {
            self.calls.lock().unwrap().push(intent_id);
            self.outcomes.lock().unwrap().pop_front().unwrap()
        }
    }
}
