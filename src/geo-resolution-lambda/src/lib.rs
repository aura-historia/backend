use application::operation_context::{OperationContext, Principal};
use aura_historia_jobs::{DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use geo_service::resolution::{ResolutionId, ResolutionWork, ResolveOutcome, ResolveUseCase};
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
/// How one wake-up left the use case. Only `Complete` acknowledges the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    Complete,
    /// Another attempt holds the resolution's claim, or this attempt lost it before completing.
    Deferred(ResolutionId),
    Failed {
        resolution_id: ResolutionId,
        failure: FailureDiagnostic,
    },
    Invalid,
}

pub async fn process_job(body: &str, use_case: &dyn ResolveUseCase) -> JobOutcome {
    let Ok(job) = decode(body, WorkerScope::GeoResolution) else {
        return JobOutcome::Invalid;
    };
    let DomainJobPayload::GeoResolution(job) = job.payload else {
        return JobOutcome::Invalid;
    };
    let work = ResolutionWork {
        id: job.resolution_id,
        generation: job.generation,
    };
    match use_case.execute(&context(), work).await {
        Ok(ResolveOutcome::Complete) => JobOutcome::Complete,
        Ok(ResolveOutcome::Deferred) => JobOutcome::Deferred(job.resolution_id),
        Err(error) => JobOutcome::Failed {
            resolution_id: job.resolution_id,
            failure: FailureDiagnostic::from_kind(
                "resolution",
                error.failure_kind(),
                Some(Dependency::AmazonLocation),
                &error,
            ),
        },
    }
}

/// Records each retained record once, keyed by its message ID; the error chain itself is never
/// logged.
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
        let (category, resolution_id, failure) = match &attempt.outcome {
            RecordOutcome::Completed(JobOutcome::Complete) => return true,
            RecordOutcome::Completed(JobOutcome::Deferred(resolution_id)) => {
                ("resolution_deferred", Some(resolution_id), None)
            }
            RecordOutcome::Completed(JobOutcome::Failed {
                resolution_id,
                failure,
            }) => ("resolution_failed", Some(resolution_id), Some(failure)),
            RecordOutcome::Completed(JobOutcome::Invalid) => ("invalid_job", None, None),
            RecordOutcome::MissingBody => ("missing_body", None, None),
            RecordOutcome::InsufficientBudget => ("insufficient_budget", None, None),
            RecordOutcome::TimedOut => ("timed_out", None, None),
            RecordOutcome::Panicked => ("panicked", None, None),
        };
        // tracing needs a literal level per call site; both levels record the same fields.
        macro_rules! retained {
            ($level:ident) => {
                tracing::$level!(
                    event = "geo_resolution.record_retained",
                    metric = "geo_resolution_record",
                    outcome = "RETRY",
                    message_id = %attempt.message_id,
                    reason = category,
                    resolution_id = resolution_id.map(tracing::field::display),
                    stage = failure.map(FailureDiagnostic::stage),
                    dependency = failure.and_then(FailureDiagnostic::dependency),
                    error_category = failure.map(FailureDiagnostic::category),
                    sqlstate = failure.and_then(FailureDiagnostic::sqlstate),
                )
            };
        }
        // Another attempt owning the claim is expected contention, not a failure.
        if matches!(
            attempt.outcome,
            RecordOutcome::Completed(JobOutcome::Deferred(_))
        ) {
            retained!(info);
        } else {
            retained!(warn);
        }
        false
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
            assert_eq!(JobOutcome::Invalid, process_job(&body, &stub).await);
        }
        assert_eq!(0, stub.calls.load(Ordering::SeqCst));
        assert_eq!(JobOutcome::Complete, process_job(JOB, &stub).await);
        let deferred = Stub {
            calls: AtomicUsize::new(0),
            result: ResolveOutcome::Deferred,
        };
        assert!(matches!(
            process_job(JOB, &deferred).await,
            JobOutcome::Deferred(_)
        ));
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
    async fn should_record_one_retained_event_per_failed_record_without_its_cause() {
        let (logs, guard) = CapturedLogs::start();

        let response = handler(event(), &FailingStorage, Duration::from_secs(30))
            .await
            .unwrap();
        drop(guard);

        assert_eq!(1, response.batch_item_failures.len());
        assert_eq!(
            1,
            logs.events_where("metric", "geo_resolution_record").len()
        );
        let retained = logs.events_where("event", "geo_resolution.record_retained");
        assert_eq!(1, retained.len());
        assert_eq!(Some("message-1"), retained[0]["message_id"].as_str());
        assert_eq!(Some("resolution_failed"), retained[0]["reason"].as_str());
        assert_eq!(
            Some("gres_01h455vb4pex5vy7enb1p677vn"),
            retained[0]["resolution_id"].as_str()
        );
        assert_eq!(Some("resolution"), retained[0]["stage"].as_str());
        assert_eq!(Some("postgres"), retained[0]["dependency"].as_str());
        assert_eq!(Some("tls"), retained[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }

    fn event() -> LambdaEvent<SqsEvent> {
        let mut message = aws_lambda_events::sqs::SqsMessage::default();
        message.message_id = Some("message-1".into());
        message.body = Some(JOB.into());
        let mut event = SqsEvent::default();
        event.records.push(message);
        LambdaEvent::new(event, lambda_runtime::Context::default())
    }
}
