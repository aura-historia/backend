use aura_historia_jobs::{DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_sqs::{RecordOutcome, process_batch};
use product_listing_service::geography::ProjectListingGeographyUseCase;
use std::time::Duration;
pub async fn process_job(body: &str, use_case: &dyn ProjectListingGeographyUseCase) -> bool {
    let Ok(job) = decode(body, WorkerScope::ListingGeography) else {
        return false;
    };
    let DomainJobPayload::ListingGeography(job) = job.payload else {
        return false;
    };
    // Wake-ups identify work, never frozen geography. Current authority/claim fences
    // govern even old generations and duplicate messages.
    use_case.execute(Some(job.work_id), 100).await.is_ok()
}
pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &dyn ProjectListingGeographyUseCase,
    budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(event, budget, Duration::from_secs(25), |body| async move {
        process_job(&body, use_case).await
    })
    .await?;
    Ok(results.finish(|attempt| matches!(attempt.outcome, RecordOutcome::Completed(true))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_service::geography::{GeographyError, GeographyWorkId};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Stub {
        calls: AtomicUsize,
        deferred: bool,
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
            if self.deferred {
                Err(GeographyError::Deferred)
            } else {
                Ok(0)
            }
        }
    }
    const JOB: &str = r#"{"schema_version":2,"scope":"listing-geography","idempotency_key":"listing-geography:gwork_01h455vb4pex5vy7enb1p677vn:1","ordering_key":"listing-geography:gwork_01h455vb4pex5vy7enb1p677vn","job_type":"LISTING_GEOGRAPHY","payload":{"work_id":"gwork_01h455vb4pex5vy7enb1p677vn","generation":1}}"#;

    #[tokio::test]
    async fn malformed_jobs_never_project_and_completed_replays_acknowledge() {
        let stub = Stub {
            calls: AtomicUsize::new(0),
            deferred: false,
        };
        for body in [
            "malformed".into(),
            JOB.replace("\"generation\":1", "\"generation\":0"),
            JOB.replace("gwork_", "usr_"),
        ] {
            assert!(!process_job(&body, &stub).await);
        }
        assert_eq!(0, stub.calls.load(Ordering::SeqCst));
        assert!(process_job(JOB, &stub).await);
        assert_eq!(1, stub.calls.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn claimed_work_retries_until_another_worker_completes() {
        let stub = Stub {
            calls: AtomicUsize::new(0),
            deferred: true,
        };
        assert!(!process_job(JOB, &stub).await);
        assert_eq!(1, stub.calls.load(Ordering::SeqCst));
    }
}
