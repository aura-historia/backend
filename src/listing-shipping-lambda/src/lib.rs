use aura_historia_jobs::{DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_bootstrap::FailureDiagnostic;
use platform_lambda_sqs::{RecordOutcome, process_batch};
use product_listing_service::shipping_projection::{
    ProjectListingShippingUseCase, ShippingProjectionError,
};
use std::time::Duration;
pub async fn process_job(body: &str, use_case: &dyn ProjectListingShippingUseCase) -> bool {
    let Ok(job) = decode(body, WorkerScope::ListingShipping) else {
        return false;
    };
    let DomainJobPayload::ListingShipping(job) = job.payload else {
        return false;
    };
    // Wake-ups identify work, never frozen shipping. Current authority/claim fences
    // govern even old generations and duplicate messages.
    match use_case.execute(job.work_id, 100).await {
        Ok(_) => true,
        Err(error) => {
            let (category, failure) = shipping_failure(&error);
            tracing::warn!(
                event = "listing_shipping.work_failed",
                work_id = %job.work_id,
                category,
                stage = failure.as_ref().map(FailureDiagnostic::stage),
                dependency = failure.as_ref().map(FailureDiagnostic::dependency),
                error_category = failure.as_ref().map(FailureDiagnostic::category),
                sqlstate = failure.as_ref().and_then(FailureDiagnostic::sqlstate),
            );
            false
        }
    }
}

/// Claim outcomes are expected contention and invalid input is the worker's own request; only
/// dependency failures carry safe cause facts.
fn shipping_failure(error: &ShippingProjectionError) -> (&'static str, Option<FailureDiagnostic>) {
    let stage = match error {
        ShippingProjectionError::Deferred => return ("DEFERRED", None),
        ShippingProjectionError::Superseded => return ("SUPERSEDED", None),
        ShippingProjectionError::InvalidInput => return ("INVALID_INPUT", None),
        ShippingProjectionError::Storage(_) => "shipping_projection",
        ShippingProjectionError::Transaction(_) => "shipping_transaction",
        ShippingProjectionError::Assessment(_) => "shipping_assessment",
    };
    (
        "DEPENDENCY_OR_STORAGE",
        Some(FailureDiagnostic::postgres(stage, error)),
    )
}
pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &dyn ProjectListingShippingUseCase,
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
    use product_listing_service::shipping_projection::{ShippingProjectionError, ShippingWorkId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_api::CapturedLogs;

    struct Stub {
        calls: AtomicUsize,
        deferred: bool,
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
            if self.deferred {
                Err(ShippingProjectionError::Deferred)
            } else {
                Ok(0)
            }
        }
    }
    const JOB: &str = r#"{"schema_version":1,"scope":"listing-shipping","idempotency_key":"listing-shipping:swork_01h455vb4pex5vy7enb1p677vn:1","ordering_key":"listing-shipping:swork_01h455vb4pex5vy7enb1p677vn","job_type":"LISTING_SHIPPING","payload":{"work_id":"swork_01h455vb4pex5vy7enb1p677vn","generation":1}}"#;

    #[tokio::test]
    async fn malformed_jobs_never_project_and_completed_replays_acknowledge() {
        let stub = Stub {
            calls: AtomicUsize::new(0),
            deferred: false,
        };
        for body in [
            "malformed".into(),
            JOB.replace("\"generation\":1", "\"generation\":0"),
            JOB.replace("swork_", "usr_"),
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
    impl ProjectListingShippingUseCase for FailingStorage {
        async fn execute(
            &self,
            _work: ShippingWorkId,
            _limit: u16,
        ) -> Result<u32, ShippingProjectionError> {
            Err(ShippingProjectionError::Storage(Box::new(
                sqlx::Error::Protocol(SENTINEL.to_owned()),
            )))
        }
    }

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let (logs, guard) = CapturedLogs::start();

        let completed = process_job(JOB, &FailingStorage).await;
        drop(guard);

        assert!(!completed);
        let failures = logs.events_where("event", "listing_shipping.work_failed");
        assert_eq!(1, failures.len());
        assert_eq!(Some("shipping_projection"), failures[0]["stage"].as_str());
        assert_eq!(Some("postgres"), failures[0]["dependency"].as_str());
        assert_eq!(Some("protocol"), failures[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }
}
