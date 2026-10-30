use aura_historia_jobs::{DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use geo_service::resolution::ResolutionError;
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_bootstrap::FailureDiagnostic;
use platform_lambda_sqs::{RecordOutcome, process_batch};
use product_listing_service::geography::{GeographyError, ProjectListingGeographyUseCase};
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
    let Err(error) = use_case.execute(Some(job.work_id), 100).await else {
        return true;
    };
    // Claim contention is expected: another worker owns or superseded this work.
    if matches!(error, GeographyError::Deferred | GeographyError::Superseded) {
        return false;
    }
    let failure = listing_geography_failure(&error);
    tracing::warn!(
        event = "listing_geography.work_failed",
        work_id = %job.work_id,
        stage = failure.stage(),
        dependency = failure.dependency(),
        error_category = failure.category(),
        sqlstate = failure.sqlstate(),
    );
    false
}

/// Keeps the failed stage and safe cause facts; the error chain itself is never logged.
fn listing_geography_failure(error: &GeographyError) -> FailureDiagnostic {
    match error {
        GeographyError::Storage(_) => FailureDiagnostic::postgres("geography_projection", error),
        GeographyError::Transaction(_) => {
            FailureDiagnostic::postgres("geography_transaction", error)
        }
        GeographyError::Resolution(error) => resolution_failure("geography_resolution", error),
        GeographyError::Superseded => {
            FailureDiagnostic::rejected("geography_projection", "claim_superseded")
        }
        GeographyError::Deferred => {
            FailureDiagnostic::rejected("geography_projection", "claimed_elsewhere")
        }
        GeographyError::InvalidInput => {
            FailureDiagnostic::rejected("geography_projection", "invalid_input")
        }
    }
}

fn resolution_failure(stage: &'static str, error: &ResolutionError) -> FailureDiagnostic {
    match error {
        ResolutionError::Provider(provider) => FailureDiagnostic::new(
            stage,
            "amazon_location",
            provider.kind.diagnostic_category(),
            None,
        ),
        ResolutionError::Storage(_) | ResolutionError::Transaction(_) => {
            FailureDiagnostic::postgres(stage, error)
        }
        ResolutionError::InvalidState(_) => {
            FailureDiagnostic::new(stage, "postgres", "invalid_persisted_state", None)
        }
        ResolutionError::InvalidInput => FailureDiagnostic::rejected(stage, "invalid_input"),
        ResolutionError::Forbidden => FailureDiagnostic::rejected(stage, "forbidden"),
        ResolutionError::Conflict => FailureDiagnostic::rejected(stage, "conflict"),
        ResolutionError::Missing => FailureDiagnostic::rejected(stage, "missing"),
    }
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
    use test_api::CapturedLogs;

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
    const JOB: &str = r#"{"schema_version":1,"scope":"listing-geography","idempotency_key":"listing-geography:gwork_01h455vb4pex5vy7enb1p677vn:1","ordering_key":"listing-geography:gwork_01h455vb4pex5vy7enb1p677vn","job_type":"LISTING_GEOGRAPHY","payload":{"work_id":"gwork_01h455vb4pex5vy7enb1p677vn","generation":1}}"#;

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

    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    struct FailingStorage;

    #[async_trait::async_trait]
    impl ProjectListingGeographyUseCase for FailingStorage {
        async fn execute(
            &self,
            _work: Option<GeographyWorkId>,
            _limit: u16,
        ) -> Result<u32, GeographyError> {
            Err(GeographyError::Storage(Box::new(sqlx::Error::Io(
                std::io::Error::other(SENTINEL),
            ))))
        }
    }

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let (logs, guard) = CapturedLogs::start();

        let completed = process_job(JOB, &FailingStorage).await;
        drop(guard);

        assert!(!completed);
        let failures = logs.events_where("event", "listing_geography.work_failed");
        assert_eq!(1, failures.len());
        assert_eq!(Some("geography_projection"), failures[0]["stage"].as_str());
        assert_eq!(Some("postgres"), failures[0]["dependency"].as_str());
        assert_eq!(Some("io"), failures[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }
}
