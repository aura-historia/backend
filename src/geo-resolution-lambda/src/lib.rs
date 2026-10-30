use application::operation_context::{OperationContext, Principal};
use aura_historia_jobs::{DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use geo_service::resolution::{ResolutionWork, ResolveOutcome, ResolveUseCase};
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_bootstrap::{Dependency, FailureDiagnostic};
use platform_lambda_sqs::{RecordOutcome, process_batch};
use std::time::Duration;

fn context() -> OperationContext {
    OperationContext {
        principal: Principal::Service("geo-resolution-worker".into()),
        request_id: "geo-resolution".into(),
        correlation_id: "geo-resolution".into(),
    }
}
pub async fn process_job(body: &str, use_case: &dyn ResolveUseCase) -> bool {
    let Ok(job) = decode(body, WorkerScope::GeoResolution) else {
        return false;
    };
    let DomainJobPayload::GeoResolution(job) = job.payload else {
        return false;
    };
    let work = ResolutionWork {
        id: job.resolution_id,
        generation: job.generation,
    };
    match use_case.execute(&context(), work).await {
        Ok(outcome) => outcome == ResolveOutcome::Complete,
        Err(error) => {
            let failure = FailureDiagnostic::from_kind(
                "resolution",
                error.failure_kind(),
                Some(Dependency::AmazonLocation),
                &error,
            );
            tracing::warn!(
                event = "geo_resolution.work_failed",
                resolution_id = %job.resolution_id,
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                sqlstate = failure.sqlstate(),
            );
            false
        }
    }
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &dyn ResolveUseCase,
    budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(event, budget, Duration::from_secs(25), |body| async move {
        process_job(&body, use_case).await
    })
    .await?;
    Ok(results.finish(|attempt| {
        let complete = matches!(attempt.outcome, RecordOutcome::Completed(true));
        if !complete {
            tracing::warn!(metric = "geo_resolution_record", outcome = "RETRY");
        }
        complete
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo_service::resolution::ResolutionError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_api::CapturedLogs;
    struct Stub {
        calls: AtomicUsize,
        result: ResolveOutcome,
    }
    #[async_trait::async_trait]
    impl ResolveUseCase for Stub {
        async fn execute(
            &self,
            context: &OperationContext,
            _: ResolutionWork,
        ) -> Result<ResolveOutcome, ResolutionError> {
            assert!(matches!(context.principal, Principal::Service(_)));
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.result)
        }
    }
    const JOB: &str = r#"{"schema_version":1,"scope":"geo-resolution","idempotency_key":"geo-resolution:gres_01h455vb4pex5vy7enb1p677vn:1","ordering_key":"geo-resolution:gres_01h455vb4pex5vy7enb1p677vn","job_type":"GEO_RESOLUTION","payload":{"resolution_id":"gres_01h455vb4pex5vy7enb1p677vn","generation":1}}"#;
    #[tokio::test]
    async fn only_confirmed_completion_acknowledges_and_malformed_jobs_never_invoke_the_use_case() {
        let stub = Stub {
            calls: AtomicUsize::new(0),
            result: ResolveOutcome::Complete,
        };
        for body in [
            "malformed".into(),
            JOB.replace("\"generation\":1", "\"generation\":0"),
            JOB.replace("gres_", "usr_"),
        ] {
            assert!(!process_job(&body, &stub).await);
        }
        assert_eq!(0, stub.calls.load(Ordering::SeqCst));
        assert!(process_job(JOB, &stub).await);
        let deferred = Stub {
            calls: AtomicUsize::new(0),
            result: ResolveOutcome::Deferred,
        };
        assert!(!process_job(JOB, &deferred).await);
    }

    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    struct FailingStorage;

    #[async_trait::async_trait]
    impl ResolveUseCase for FailingStorage {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ResolutionWork,
        ) -> Result<ResolveOutcome, ResolutionError> {
            Err(ResolutionError::Storage(Box::new(sqlx::Error::Tls(
                Box::new(std::io::Error::other(SENTINEL)),
            ))))
        }
    }

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let (logs, guard) = CapturedLogs::start();

        let completed = process_job(JOB, &FailingStorage).await;
        drop(guard);

        assert!(!completed);
        let failures = logs.events_where("event", "geo_resolution.work_failed");
        assert_eq!(1, failures.len());
        assert_eq!(Some("resolution"), failures[0]["stage"].as_str());
        assert_eq!(Some("postgres"), failures[0]["dependency"].as_str());
        assert_eq!(Some("tls"), failures[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }
}
