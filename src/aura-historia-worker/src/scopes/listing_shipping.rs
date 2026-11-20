//! `listing-shipping`: projects the effective shipping of the listings one work item names.

use crate::composition::CompositionError;
use async_trait::async_trait;
use aura_historia_jobs::{DomainJobPayload, WorkerScope, decode};
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::FailureDiagnostic;
use platform_worker::{Job, JobHandler, error_class};
use product_listing_postgres::shipping_projection::SqlxShippingProjectionStoreFactory;
use product_listing_service::shipping_projection::{
    ProjectListingShippingHandler, ProjectListingShippingUseCase, ShippingProjectionError,
    ShippingWorkId,
};
use sqlx::PgPool;
use std::sync::Arc;

/// How one wake-up left the use case. Only `Complete` acknowledges the job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    Complete,
    /// Claim contention is expected: another worker owns (`DEFERRED`) or superseded
    /// (`SUPERSEDED`) this work.
    Contended(&'static str),
    Failed {
        work_id: ShippingWorkId,
        category: &'static str,
        failure: FailureDiagnostic,
    },
    Invalid,
}

pub async fn process_job(body: &str, use_case: &dyn ProjectListingShippingUseCase) -> JobOutcome {
    let Ok(job) = decode(body, WorkerScope::ListingShipping) else {
        return JobOutcome::Invalid;
    };
    let DomainJobPayload::ListingShipping(job) = job.payload else {
        return JobOutcome::Invalid;
    };
    // Wake-ups identify work, never frozen shipping. Current authority/claim fences
    // govern even old generations and duplicate jobs.
    let Err(error) = use_case.execute(job.work_id, 100).await else {
        return JobOutcome::Complete;
    };
    match error {
        ShippingProjectionError::Deferred => JobOutcome::Contended("DEFERRED"),
        ShippingProjectionError::Superseded => JobOutcome::Contended("SUPERSEDED"),
        error => {
            let (category, failure) = shipping_failure(&error);
            JobOutcome::Failed {
                work_id: job.work_id,
                category,
                failure,
            }
        }
    }
}

/// Invalid input and claim outcomes are the worker's own refusals, so they name no dependency.
fn shipping_failure(error: &ShippingProjectionError) -> (&'static str, FailureDiagnostic) {
    let rejected = |category, label| {
        (
            category,
            FailureDiagnostic::rejected("shipping_projection", label),
        )
    };
    let stage = match error {
        ShippingProjectionError::InvalidInput => return rejected("INVALID_INPUT", "invalid_input"),
        ShippingProjectionError::Deferred => return rejected("DEFERRED", "claimed_elsewhere"),
        ShippingProjectionError::Superseded => return rejected("SUPERSEDED", "claim_superseded"),
        ShippingProjectionError::Storage(_) => "shipping_projection",
        ShippingProjectionError::Transaction(_) => "shipping_transaction",
        ShippingProjectionError::Assessment(_) => "shipping_assessment",
    };
    (
        "DEPENDENCY_OR_STORAGE",
        FailureDiagnostic::postgres(stage, error),
    )
}

/// Only completion acknowledges; every other outcome is retried under its category. A failure
/// records its stage and safe cause facts once; the error chain itself is never logged.
pub fn disposition(job: &Job, outcome: &JobOutcome) -> Disposition {
    match outcome {
        JobOutcome::Complete => Disposition::Complete,
        JobOutcome::Contended(category) => Disposition::Retry(error_class(category)),
        JobOutcome::Failed {
            work_id,
            category,
            failure,
        } => {
            tracing::warn!(
                event = "listing_shipping.work_failed",
                job_id = %job.id(),
                attempt = job.attempt(),
                work_id = %work_id,
                category = *category,
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                sqlstate = failure.sqlstate(),
            );
            Disposition::Retry(error_class(category))
        }
        JobOutcome::Invalid => Disposition::Retry(error_class("invalid_job")),
    }
}

pub struct ListingShippingJobs {
    use_case: Arc<dyn ProjectListingShippingUseCase>,
}

impl ListingShippingJobs {
    pub fn new(use_case: Arc<dyn ProjectListingShippingUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for ListingShippingJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let outcome = process_job(job.payload(), self.use_case.as_ref()).await;
        disposition(job, &outcome)
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    let use_case: Arc<dyn ProjectListingShippingUseCase> =
        Arc::new(ProjectListingShippingHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxShippingProjectionStoreFactory,
        ));
    Ok(Arc::new(ListingShippingJobs::new(use_case)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_api::CapturedLogs;

    struct Stub {
        calls: AtomicUsize,
        result: fn() -> Result<u32, ShippingProjectionError>,
    }

    #[async_trait::async_trait]
    impl ProjectListingShippingUseCase for Stub {
        async fn execute(
            &self,
            _work: ShippingWorkId,
            limit: u16,
        ) -> Result<u32, ShippingProjectionError> {
            assert_eq!(100, limit);
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
    }

    fn stub(result: fn() -> Result<u32, ShippingProjectionError>) -> Arc<Stub> {
        Arc::new(Stub {
            calls: AtomicUsize::new(0),
            result,
        })
    }

    const JOB: &str = r#"{"schema_version":1,"scope":"listing-shipping","idempotency_key":"listing-shipping:swork_01h455vb4pex5vy7enb1p677vn:1","ordering_key":"listing-shipping:swork_01h455vb4pex5vy7enb1p677vn","job_type":"LISTING_SHIPPING","payload":{"work_id":"swork_01h455vb4pex5vy7enb1p677vn","generation":1}}"#;

    #[tokio::test]
    async fn malformed_jobs_never_project_and_completed_replays_acknowledge() {
        let completing = stub(|| Ok(0));
        let handler = ListingShippingJobs::new(completing.clone());
        for body in [
            "malformed".into(),
            JOB.replace("\"generation\":1", "\"generation\":0"),
            JOB.replace("swork_", "usr_"),
        ] {
            assert_eq!(JobOutcome::Invalid, process_job(&body, &*completing).await);
            assert_eq!(
                Disposition::Retry(error_class("invalid_job")),
                handler.handle(&claimed_job(&body)).await
            );
        }
        assert_eq!(0, completing.calls.load(Ordering::SeqCst));
        assert_eq!(JobOutcome::Complete, process_job(JOB, &*completing).await);
        assert_eq!(1, completing.calls.load(Ordering::SeqCst));
        assert_eq!(
            Disposition::Complete,
            handler.handle(&claimed_job(JOB)).await
        );
        assert_eq!(2, completing.calls.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn claimed_work_retries_until_another_worker_completes_without_a_failure_event() {
        for (result, expected) in [
            (
                (|| Err(ShippingProjectionError::Deferred)) as fn() -> _,
                Disposition::Retry(error_class("DEFERRED")),
            ),
            (
                || Err(ShippingProjectionError::Superseded),
                Disposition::Retry(error_class("SUPERSEDED")),
            ),
        ] {
            let contended = stub(result);
            let (logs, guard) = CapturedLogs::start();

            let disposition = ListingShippingJobs::new(contended.clone())
                .handle(&claimed_job(JOB))
                .await;
            drop(guard);

            assert_eq!(expected, disposition);
            assert_eq!(1, contended.calls.load(Ordering::SeqCst));
            assert!(
                logs.events_where("event", "listing_shipping.work_failed")
                    .is_empty()
            );
        }
    }

    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let (logs, guard) = CapturedLogs::start();
        let job = claimed_job(JOB);
        let failing = stub(|| {
            Err(ShippingProjectionError::Storage(Box::new(
                sqlx::Error::Protocol(SENTINEL.to_owned()),
            )))
        });

        let disposition = ListingShippingJobs::new(failing).handle(&job).await;
        drop(guard);

        assert_eq!(
            Disposition::Retry(error_class("DEPENDENCY_OR_STORAGE")),
            disposition
        );
        let failures = logs.events_where("event", "listing_shipping.work_failed");
        assert_eq!(1, failures.len());
        assert_eq!(
            Some(job.id().to_string().as_str()),
            failures[0]["job_id"].as_str()
        );
        assert_eq!(
            Some("swork_01h455vb4pex5vy7enb1p677vn"),
            failures[0]["work_id"].as_str()
        );
        assert_eq!(
            Some("DEPENDENCY_OR_STORAGE"),
            failures[0]["category"].as_str()
        );
        assert_eq!(Some("shipping_projection"), failures[0]["stage"].as_str());
        assert_eq!(Some("postgres"), failures[0]["dependency"].as_str());
        assert_eq!(Some("protocol"), failures[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }

    #[tokio::test]
    async fn should_retry_rejected_input_without_naming_a_dependency() {
        let (logs, guard) = CapturedLogs::start();

        let disposition =
            ListingShippingJobs::new(stub(|| Err(ShippingProjectionError::InvalidInput)))
                .handle(&claimed_job(JOB))
                .await;
        drop(guard);

        assert_eq!(
            Disposition::Retry(error_class("INVALID_INPUT")),
            disposition
        );
        let failures = logs.events_where("event", "listing_shipping.work_failed");
        assert_eq!(1, failures.len());
        assert_eq!(Some("INVALID_INPUT"), failures[0]["category"].as_str());
        assert_eq!(
            Some("invalid_input"),
            failures[0]["error_category"].as_str()
        );
        assert!(failures[0].get("dependency").is_none());
    }
}
