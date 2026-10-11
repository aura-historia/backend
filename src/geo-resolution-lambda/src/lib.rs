use application::operation_context::{OperationContext, Principal};
use aura_historia_jobs::{DomainJobPayload, SearchFilterOperation, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use geo_service::resolution::{ResolutionWork, ResolveOutcome, ResolveUseCase};
use lambda_runtime::{Error, LambdaEvent};
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
    let Ok(job) = decode::<SearchFilterOperation>(body, WorkerScope::GeoResolution) else {
        return false;
    };
    let DomainJobPayload::GeoResolution(job) = job.payload else {
        return false;
    };
    matches!(
        use_case
            .execute(
                &context(),
                ResolutionWork {
                    id: job.resolution_id,
                    generation: job.generation
                }
            )
            .await,
        Ok(ResolveOutcome::Complete)
    )
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
    const JOB: &str = r#"{"schema_version":2,"scope":"geo-resolution","idempotency_key":"geo-resolution:gres_01h455vb4pex5vy7enb1p677vn:1","ordering_key":"geo-resolution:gres_01h455vb4pex5vy7enb1p677vn","job_type":"GEO_RESOLUTION","payload":{"resolution_id":"gres_01h455vb4pex5vy7enb1p677vn","generation":1}}"#;
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
}
