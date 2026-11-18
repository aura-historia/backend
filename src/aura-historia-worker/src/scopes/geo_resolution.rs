//! `geo-resolution`: resolves one geographic resolution generation through Amazon Location.

use crate::composition::CompositionError;
use application::operation_context::{OperationContext, Principal};
use async_trait::async_trait;
use aura_historia_jobs::{DomainJobPayload, WorkerScope, decode};
use aws_config::BehaviorVersion;
use geo_amazon_location::{AmazonLocationConfig, AmazonLocationGeocoder, REGION, TERMS_PROFILE};
use geo_postgres::SqlxResolutionRepositoryFactory;
use geo_service::{
    geocoding::ResultRetention,
    resolution::{ResolutionId, ResolutionWork, ResolveHandler, ResolveOutcome, ResolveUseCase},
};
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::{Dependency, FailureDiagnostic};
use platform_worker::{Job, JobHandler, error_class};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};

const SCOPE: &str = "geo-resolution";

fn context() -> OperationContext {
    OperationContext {
        principal: Principal::Service("geo-resolution-worker".into()),
        request_id: "geo-resolution".into(),
        correlation_id: "geo-resolution".into(),
    }
}

/// How one wake-up left the use case. Only `Complete` acknowledges the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    /// The generation is settled, including terminal and stale work; PostgreSQL holds the rest.
    Complete,
    /// The provider circuit is open. Custody stays with the queue; cleanup recovery republishes
    /// the work either way.
    ProviderGated(ResolutionId),
    /// Another attempt holds the claim or the retry backoff runs.
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
        Ok(ResolveOutcome::ProviderGated) => JobOutcome::ProviderGated(job.resolution_id),
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

/// Only completion acknowledges; every other outcome is retried. Each retried job is recorded
/// once, keyed by its job ID; the error chain itself is never logged.
pub fn disposition(job: &Job, outcome: &JobOutcome) -> Disposition {
    let (category, resolution_id, failure) = match outcome {
        JobOutcome::Complete => return Disposition::Complete,
        JobOutcome::ProviderGated(resolution_id) => ("provider_gated", Some(resolution_id), None),
        JobOutcome::Deferred(resolution_id) => ("resolution_deferred", Some(resolution_id), None),
        JobOutcome::Failed {
            resolution_id,
            failure,
        } => ("resolution_failed", Some(resolution_id), Some(failure)),
        JobOutcome::Invalid => ("invalid_job", None, None),
    };
    // tracing needs a literal level per call site; both levels record the same fields.
    macro_rules! retained {
        ($level:ident) => {
            tracing::$level!(
                event = "geo_resolution.record_retained",
                metric = "geo_resolution_record",
                outcome = "RETRY",
                job_id = %job.id(),
                attempt = job.attempt(),
                reason = category,
                resolution_id = resolution_id.map(tracing::field::display),
                stage = failure.map(FailureDiagnostic::stage),
                dependency = failure.and_then(FailureDiagnostic::dependency),
                error_category = failure.map(FailureDiagnostic::category),
                sqlstate = failure.and_then(FailureDiagnostic::sqlstate),
            )
        };
    }
    // Contention and an open provider circuit are expected states, not failures.
    if matches!(
        outcome,
        JobOutcome::Deferred(_) | JobOutcome::ProviderGated(_)
    ) {
        retained!(info);
    } else {
        retained!(warn);
    }
    Disposition::Retry(error_class(category))
}

pub struct GeoResolutionJobs {
    use_case: Arc<dyn ResolveUseCase>,
}

impl GeoResolutionJobs {
    pub fn new(use_case: Arc<dyn ResolveUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for GeoResolutionJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let outcome = process_job(job.payload(), self.use_case.as_ref()).await;
        disposition(job, &outcome)
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    let config = aws_config::defaults(BehaviorVersion::v2026_11_10())
        .region(aws_config::Region::new(REGION))
        .load()
        .await;
    // One SDK attempt per claim; durable retries own the charging bound.
    let bounds = AmazonLocationConfig::new(Duration::from_secs(6), Duration::from_secs(2), 1)
        .map_err(|error| CompositionError::new(SCOPE, error))?
        .with_retention(ResultRetention::Storage);
    let provider = Arc::new(
        AmazonLocationGeocoder::new(aws_sdk_geoplaces::Client::new(&config), bounds)
            .map_err(|error| CompositionError::new(SCOPE, error))?,
    );
    let use_case: Arc<dyn ResolveUseCase> = Arc::new(ResolveHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxResolutionRepositoryFactory,
        provider,
        TERMS_PROFILE.into(),
    ));
    Ok(Arc::new(GeoResolutionJobs::new(use_case)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
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

    fn stub(result: ResolveOutcome) -> Stub {
        Stub {
            calls: AtomicUsize::new(0),
            result,
        }
    }

    const JOB: &str = r#"{"schema_version":1,"scope":"geo-resolution","idempotency_key":"geo-resolution:gres_01h455vb4pex5vy7enb1p677vn:1","ordering_key":"geo-resolution:gres_01h455vb4pex5vy7enb1p677vn","job_type":"GEO_RESOLUTION","payload":{"resolution_id":"gres_01h455vb4pex5vy7enb1p677vn","generation":1}}"#;

    #[tokio::test]
    async fn only_confirmed_completion_acknowledges_and_malformed_jobs_never_invoke_the_use_case() {
        let complete = stub(ResolveOutcome::Complete);
        for body in [
            "malformed".into(),
            JOB.replace("\"generation\":1", "\"generation\":0"),
            JOB.replace("gres_", "usr_"),
        ] {
            assert_eq!(JobOutcome::Invalid, process_job(&body, &complete).await);
        }
        assert_eq!(0, complete.calls.load(Ordering::SeqCst));
        assert_eq!(JobOutcome::Complete, process_job(JOB, &complete).await);
        assert!(matches!(
            process_job(JOB, &stub(ResolveOutcome::Deferred)).await,
            JobOutcome::Deferred(_)
        ));
        assert!(matches!(
            process_job(JOB, &stub(ResolveOutcome::ProviderGated)).await,
            JobOutcome::ProviderGated(_)
        ));
    }

    #[tokio::test]
    async fn should_acknowledge_only_completion_and_retry_every_other_outcome() {
        let job = claimed_job(JOB);
        for (result, expected) in [
            (ResolveOutcome::Complete, Disposition::Complete),
            (
                ResolveOutcome::Deferred,
                Disposition::Retry(error_class("resolution_deferred")),
            ),
            (
                ResolveOutcome::ProviderGated,
                Disposition::Retry(error_class("provider_gated")),
            ),
        ] {
            let handler = GeoResolutionJobs::new(Arc::new(stub(result)));
            assert_eq!(expected, handler.handle(&job).await, "{result:?}");
        }
        let handler = GeoResolutionJobs::new(Arc::new(stub(ResolveOutcome::Complete)));
        assert_eq!(
            Disposition::Retry(error_class("invalid_job")),
            handler.handle(&claimed_job("malformed")).await
        );
    }

    #[tokio::test]
    async fn should_retain_provider_gated_records_at_info_with_their_reason() {
        let (logs, guard) = CapturedLogs::start();
        let handler = GeoResolutionJobs::new(Arc::new(stub(ResolveOutcome::ProviderGated)));

        let disposition = handler.handle(&claimed_job(JOB)).await;
        drop(guard);

        assert!(matches!(disposition, Disposition::Retry(_)));
        let retained = logs.events_where("event", "geo_resolution.record_retained");
        assert_eq!(1, retained.len());
        assert_eq!(Some("provider_gated"), retained[0]["reason"].as_str());
        assert_eq!(
            Some("gres_01h455vb4pex5vy7enb1p677vn"),
            retained[0]["resolution_id"].as_str()
        );
        assert!(retained[0].get("error_category").is_none());
        assert!(
            logs.text()
                .lines()
                .any(|line| line.contains("provider_gated") && line.contains(r#""level":"INFO""#))
        );
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
    async fn should_record_one_retained_event_per_failed_job_without_its_cause() {
        let (logs, guard) = CapturedLogs::start();
        let job = claimed_job(JOB);

        let disposition = GeoResolutionJobs::new(Arc::new(FailingStorage))
            .handle(&job)
            .await;
        drop(guard);

        assert_eq!(
            Disposition::Retry(error_class("resolution_failed")),
            disposition
        );
        assert_eq!(
            1,
            logs.events_where("metric", "geo_resolution_record").len()
        );
        let retained = logs.events_where("event", "geo_resolution.record_retained");
        assert_eq!(1, retained.len());
        assert_eq!(
            Some(job.id().to_string().as_str()),
            retained[0]["job_id"].as_str()
        );
        assert_eq!(Some("resolution_failed"), retained[0]["reason"].as_str());
        assert_eq!(Some("resolution"), retained[0]["stage"].as_str());
        assert_eq!(Some("postgres"), retained[0]["dependency"].as_str());
        assert_eq!(Some("tls"), retained[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }
}
