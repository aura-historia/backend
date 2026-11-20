//! `listing-geography`: projects the effective geography of the listings one work item names.

use crate::composition::CompositionError;
use async_trait::async_trait;
use aura_historia_jobs::{DomainJobPayload, WorkerScope, decode};
use geo_postgres::SqlxResolutionRepositoryFactory;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::{Dependency, FailureDiagnostic};
use platform_worker::{Job, JobHandler, error_class};
use product_listing_postgres::geography::SqlxGeographyProjectionStoreFactory;
use product_listing_service::geography::{
    GeographyError, GeographyWorkId, ProjectListingGeographyHandler, ProjectListingGeographyUseCase,
};
use sqlx::PgPool;
use std::sync::Arc;

/// How one wake-up left the use case. Only `Complete` acknowledges the job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    Complete,
    /// Claim contention is expected: another worker owns (`deferred`) or superseded
    /// (`superseded`) this work.
    Contended(&'static str),
    Failed {
        work_id: GeographyWorkId,
        failure: FailureDiagnostic,
    },
    Invalid,
}

pub async fn process_job(body: &str, use_case: &dyn ProjectListingGeographyUseCase) -> JobOutcome {
    let Ok(job) = decode(body, WorkerScope::ListingGeography) else {
        return JobOutcome::Invalid;
    };
    let DomainJobPayload::ListingGeography(job) = job.payload else {
        return JobOutcome::Invalid;
    };
    // Wake-ups identify work, never frozen geography. Current authority/claim fences
    // govern even old generations and duplicate jobs.
    let Err(error) = use_case.execute(Some(job.work_id), 100).await else {
        return JobOutcome::Complete;
    };
    match error {
        GeographyError::Deferred => JobOutcome::Contended("deferred"),
        GeographyError::Superseded => JobOutcome::Contended("superseded"),
        error => JobOutcome::Failed {
            work_id: job.work_id,
            failure: FailureDiagnostic::from_kind(
                error.failure_stage(),
                error.failure_kind(),
                Some(Dependency::AmazonLocation),
                &error,
            ),
        },
    }
}

/// Only completion acknowledges; every other outcome is retried. A failure records its stage
/// and safe cause facts once; the error chain itself is never logged.
pub fn disposition(job: &Job, outcome: &JobOutcome) -> Disposition {
    match outcome {
        JobOutcome::Complete => Disposition::Complete,
        JobOutcome::Contended(category) => Disposition::Retry(error_class(category)),
        JobOutcome::Failed { work_id, failure } => {
            tracing::warn!(
                event = "listing_geography.work_failed",
                job_id = %job.id(),
                attempt = job.attempt(),
                work_id = %work_id,
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                sqlstate = failure.sqlstate(),
            );
            Disposition::Retry(error_class("work_failed"))
        }
        JobOutcome::Invalid => Disposition::Retry(error_class("invalid_job")),
    }
}

pub struct ListingGeographyJobs {
    use_case: Arc<dyn ProjectListingGeographyUseCase>,
}

impl ListingGeographyJobs {
    pub fn new(use_case: Arc<dyn ProjectListingGeographyUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for ListingGeographyJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let outcome = process_job(job.payload(), self.use_case.as_ref()).await;
        disposition(job, &outcome)
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    let use_case: Arc<dyn ProjectListingGeographyUseCase> =
        Arc::new(ProjectListingGeographyHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxGeographyProjectionStoreFactory,
            SqlxResolutionRepositoryFactory,
        ));
    Ok(Arc::new(ListingGeographyJobs::new(use_case)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_api::CapturedLogs;

    struct Stub {
        calls: AtomicUsize,
        result: fn() -> Result<u32, GeographyError>,
    }

    #[async_trait::async_trait]
    impl ProjectListingGeographyUseCase for Stub {
        async fn execute(
            &self,
            work: Option<GeographyWorkId>,
            limit: u16,
        ) -> Result<u32, GeographyError> {
            assert!(work.is_some());
            assert_eq!(100, limit);
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
    }

    fn stub(result: fn() -> Result<u32, GeographyError>) -> Stub {
        Stub {
            calls: AtomicUsize::new(0),
            result,
        }
    }

    const JOB: &str = r#"{"schema_version":1,"scope":"listing-geography","idempotency_key":"listing-geography:gwork_01h455vb4pex5vy7enb1p677vn:1","ordering_key":"listing-geography:gwork_01h455vb4pex5vy7enb1p677vn","job_type":"LISTING_GEOGRAPHY","payload":{"work_id":"gwork_01h455vb4pex5vy7enb1p677vn","generation":1}}"#;

    #[tokio::test]
    async fn malformed_jobs_never_project_and_completed_replays_acknowledge() {
        let completing = Arc::new(stub(|| Ok(0)));
        let handler = ListingGeographyJobs::new(completing.clone());
        for body in [
            "malformed".into(),
            JOB.replace("\"generation\":1", "\"generation\":0"),
            JOB.replace("gwork_", "usr_"),
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
    async fn claimed_work_retries_until_another_worker_completes() {
        for (result, expected) in [
            (
                (|| Err(GeographyError::Deferred)) as fn() -> _,
                Disposition::Retry(error_class("deferred")),
            ),
            (
                || Err(GeographyError::Superseded),
                Disposition::Retry(error_class("superseded")),
            ),
        ] {
            let (logs, guard) = CapturedLogs::start();
            let stub = Arc::new(stub(result));
            let handler = ListingGeographyJobs::new(stub.clone());

            let disposition = handler.handle(&claimed_job(JOB)).await;
            drop(guard);

            assert_eq!(expected, disposition);
            assert_eq!(1, stub.calls.load(Ordering::SeqCst));
            assert!(
                logs.events_where("event", "listing_geography.work_failed")
                    .is_empty()
            );
        }
    }

    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let (logs, guard) = CapturedLogs::start();
        let job = claimed_job(JOB);
        let handler = ListingGeographyJobs::new(Arc::new(stub(|| {
            Err(GeographyError::Storage(Box::new(sqlx::Error::Io(
                std::io::Error::other(SENTINEL),
            ))))
        })));

        let disposition = handler.handle(&job).await;
        drop(guard);

        assert_eq!(Disposition::Retry(error_class("work_failed")), disposition);
        let failures = logs.events_where("event", "listing_geography.work_failed");
        assert_eq!(1, failures.len());
        assert_eq!(
            Some(job.id().to_string().as_str()),
            failures[0]["job_id"].as_str()
        );
        assert_eq!(
            Some("gwork_01h455vb4pex5vy7enb1p677vn"),
            failures[0]["work_id"].as_str()
        );
        assert_eq!(Some("geography_projection"), failures[0]["stage"].as_str());
        assert_eq!(Some("postgres"), failures[0]["dependency"].as_str());
        assert_eq!(Some("io"), failures[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }

    #[tokio::test]
    async fn should_retry_rejected_input_as_a_failed_work_item() {
        let handler =
            ListingGeographyJobs::new(Arc::new(stub(|| Err(GeographyError::InvalidInput))));

        assert_eq!(
            Disposition::Retry(error_class("work_failed")),
            handler.handle(&claimed_job(JOB)).await
        );
    }
}
