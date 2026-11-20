//! `user-identity-deletion`: removes a deleted user's Cognito identity.

use crate::composition::CompositionError;
use async_trait::async_trait;
use aura_historia_jobs::{DomainJobPayload, WorkerScope, decode};
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::{Dependency, FailureDiagnostic, INVALID_PERSISTED_STATE, config};
use platform_worker::{Job, JobHandler, error_class};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use user_cognito::CognitoUserIdentityDeleter;
use user_postgres::SqlxUserIdentityDeletionStoreFactory;
use user_service::ports::user_identity_deletion::UserIdentityDeletionError;
use user_service::use_cases::commands::delete_user_identity::{
    DeleteUserIdentityError, DeleteUserIdentityHandler, DeleteUserIdentityResult,
    DeleteUserIdentityUseCase,
};

const SCOPE: &str = "user-identity-deletion";
const USER_POOL_ID_ENV: &str = "AURA_HISTORIA_COGNITO_USER_POOL_ID";

#[derive(Debug, Clone, PartialEq, Eq)]
enum JobOutcome {
    Completed,
    /// Another attempt owns the deletion: its lease is active, or it took over this claim.
    Contended(&'static str),
    Retry(FailureDiagnostic),
    Invalid,
}

async fn process_job(body: &str, use_case: &dyn DeleteUserIdentityUseCase) -> JobOutcome {
    let Ok(job) = decode(body, WorkerScope::UserIdentityDeletion) else {
        return JobOutcome::Invalid;
    };
    let DomainJobPayload::UserIdentityDeletion(job) = job.payload else {
        return JobOutcome::Invalid;
    };
    match use_case.execute(job.user_id).await {
        Ok(DeleteUserIdentityResult::Completed) => JobOutcome::Completed,
        Ok(DeleteUserIdentityResult::Deferred) => JobOutcome::Contended("active_lease"),
        Err(error) => failure_outcome(&error),
    }
}

/// Keeps the failed step and safe cause facts; the error chain itself is never logged.
fn failure_outcome(error: &DeleteUserIdentityError) -> JobOutcome {
    use DeleteUserIdentityError as E;
    let (stage, cause) = match error {
        E::Claim(cause) => ("claim", cause),
        E::TargetLookup(cause) => ("target_lookup", cause),
        E::TargetNotYetObserved => {
            return JobOutcome::Retry(FailureDiagnostic::new(
                "target_lookup",
                Dependency::Cognito,
                "not_yet_observed",
                None,
            ));
        }
        E::TargetSave(cause) => ("target_save", cause),
        E::IdentityRemoval(cause) => ("identity_removal", cause),
        E::Completion(cause) => ("completion", cause),
    };
    JobOutcome::Retry(match cause {
        UserIdentityDeletionError::Storage(_) => FailureDiagnostic::postgres(stage, error),
        UserIdentityDeletionError::InvalidPersistedState(_) => {
            FailureDiagnostic::new(stage, Dependency::Postgres, INVALID_PERSISTED_STATE, None)
        }
        // Another attempt took over the lease and completes the deletion.
        UserIdentityDeletionError::ClaimLost => return JobOutcome::Contended("claim_lost"),
        UserIdentityDeletionError::IdentityProvider {
            failure,
            http_status,
            ..
        } => FailureDiagnostic::new(
            stage,
            Dependency::Cognito,
            failure.diagnostic_category(),
            *http_status,
        ),
    })
}

/// Only committed completion acknowledges; every other outcome is retried. Each retried job is
/// recorded once, keyed by its job ID.
fn disposition(job: &Job, outcome: &JobOutcome) -> Disposition {
    let (category, failure) = match outcome {
        JobOutcome::Completed => return Disposition::Complete,
        JobOutcome::Contended(category) => {
            // Expected contention, not a failure: the other attempt completes the deletion.
            tracing::info!(
                event = "user.identity_deletion.retained",
                job_id = %job.id(),
                attempt = job.attempt(),
                outcome = *category,
                "User identity deletion retained while another attempt owns it",
            );
            return Disposition::Retry(error_class(category));
        }
        JobOutcome::Retry(failure) => ("deletion_unconfirmed", Some(failure)),
        JobOutcome::Invalid => ("invalid_job", None),
    };
    tracing::warn!(
        event = "user.identity_deletion.retained",
        job_id = %job.id(),
        attempt = job.attempt(),
        outcome = category,
        stage = failure.map(FailureDiagnostic::stage),
        dependency = failure.and_then(FailureDiagnostic::dependency),
        error_category = failure.map(FailureDiagnostic::category),
        http_status = failure.and_then(FailureDiagnostic::http_status),
        sqlstate = failure.and_then(FailureDiagnostic::sqlstate),
        "User identity deletion retained for retry",
    );
    Disposition::Retry(error_class(category))
}

pub struct UserIdentityDeletionJobs {
    use_case: Arc<dyn DeleteUserIdentityUseCase>,
}

impl UserIdentityDeletionJobs {
    pub fn new(use_case: Arc<dyn DeleteUserIdentityUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for UserIdentityDeletionJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let outcome = process_job(job.payload(), self.use_case.as_ref()).await;
        disposition(job, &outcome)
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    let user_pool_id =
        config::required(USER_POOL_ID_ENV).map_err(|error| CompositionError::new(SCOPE, error))?;
    let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let cognito_config = aws_sdk_cognitoidentityprovider::config::Builder::from(&aws_config)
        .timeout_config(
            aws_sdk_cognitoidentityprovider::config::timeout::TimeoutConfig::builder()
                .operation_timeout(Duration::from_secs(3))
                .build(),
        )
        .build();
    let cognito = aws_sdk_cognitoidentityprovider::Client::from_conf(cognito_config);
    let use_case: Arc<dyn DeleteUserIdentityUseCase> = Arc::new(DeleteUserIdentityHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxUserIdentityDeletionStoreFactory,
        CognitoUserIdentityDeleter::new(cognito, user_pool_id),
    ));
    Ok(Arc::new(UserIdentityDeletionJobs::new(use_case)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_api::CapturedLogs;
    use user_core::user_id::UserId;
    use user_service::ports::user_identity_deletion::IdentityProviderFailure;

    const JOB: &str = r#"{"schema_version":1,"scope":"user-identity-deletion","idempotency_key":"user-identity-deletion:usr_01h455vb4pex5vy7enb1p677vn","ordering_key":"user-identity-deletion:usr_01h455vb4pex5vy7enb1p677vn","job_type":"USER_IDENTITY_DELETION","payload":{"user_id":"usr_01h455vb4pex5vy7enb1p677vn"}}"#;

    struct Stub {
        outcome: u8,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl DeleteUserIdentityUseCase for Stub {
        async fn execute(
            &self,
            _: UserId,
        ) -> Result<DeleteUserIdentityResult, DeleteUserIdentityError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.outcome {
                0 => Ok(DeleteUserIdentityResult::Completed),
                1 => Ok(DeleteUserIdentityResult::Deferred),
                _ => Err(DeleteUserIdentityError::IdentityRemoval(
                    UserIdentityDeletionError::IdentityProvider {
                        failure: IdentityProviderFailure::Unavailable,
                        http_status: Some(503),
                        source: application::error::static_error("provider unavailable"),
                    },
                )),
            }
        }
    }

    fn stub(outcome: u8) -> Arc<Stub> {
        Arc::new(Stub {
            outcome,
            calls: AtomicUsize::new(0),
        })
    }

    #[tokio::test]
    async fn only_committed_completion_acknowledges_the_job() {
        for (outcome, expected) in [
            (0, Disposition::Complete),
            (1, Disposition::Retry(error_class("active_lease"))),
            (2, Disposition::Retry(error_class("deletion_unconfirmed"))),
        ] {
            let stub = stub(outcome);
            let disposition = UserIdentityDeletionJobs::new(stub.clone())
                .handle(&claimed_job(JOB))
                .await;
            assert_eq!(expected, disposition, "{outcome}");
            assert_eq!(1, stub.calls.load(Ordering::SeqCst));
        }
    }

    #[tokio::test]
    async fn invalid_jobs_and_wrong_scope_never_call_the_use_case() {
        let stub = stub(0);
        let handler = UserIdentityDeletionJobs::new(stub.clone());
        for body in [
            "bad body".to_owned(),
            JOB.replace("usr_", "mci_"),
            JOB.replace(
                "\"scope\":\"user-identity-deletion\"",
                "\"scope\":\"notification-delivery\"",
            ),
        ] {
            assert_eq!(JobOutcome::Invalid, process_job(&body, &*stub).await);
            assert_eq!(
                Disposition::Retry(error_class("invalid_job")),
                handler.handle(&claimed_job(&body)).await
            );
        }
        assert_eq!(0, stub.calls.load(Ordering::SeqCst));
    }

    /// A use case that fails once with the given error.
    struct Failing(std::sync::Mutex<Option<DeleteUserIdentityError>>);

    #[async_trait::async_trait]
    impl DeleteUserIdentityUseCase for Failing {
        async fn execute(
            &self,
            _: UserId,
        ) -> Result<DeleteUserIdentityResult, DeleteUserIdentityError> {
            Err(self.0.lock().unwrap().take().expect("one execution"))
        }
    }

    fn failing(error: DeleteUserIdentityError) -> UserIdentityDeletionJobs {
        UserIdentityDeletionJobs::new(Arc::new(Failing(std::sync::Mutex::new(Some(error)))))
    }

    #[derive(Debug, thiserror::Error)]
    #[error("SENTINEL_PAYLOAD_TOKEN_person@example.com")]
    struct Sentinel;

    #[tokio::test]
    async fn should_record_the_failed_step_and_system_without_logging_the_cause() {
        let sentinel = || application::error::box_error(Sentinel);
        let cases = [
            (
                DeleteUserIdentityError::Claim(UserIdentityDeletionError::Storage(Box::new(
                    sqlx::Error::PoolTimedOut,
                ))),
                ("claim", "postgres", "pool_timed_out", None),
            ),
            (
                DeleteUserIdentityError::TargetLookup(
                    UserIdentityDeletionError::IdentityProvider {
                        failure: IdentityProviderFailure::Throttled,
                        http_status: Some(400),
                        source: sentinel(),
                    },
                ),
                ("target_lookup", "cognito", "throttled", Some(400)),
            ),
            (
                DeleteUserIdentityError::TargetNotYetObserved,
                ("target_lookup", "cognito", "not_yet_observed", None),
            ),
            (
                DeleteUserIdentityError::IdentityRemoval(
                    UserIdentityDeletionError::IdentityProvider {
                        failure: IdentityProviderFailure::Authentication,
                        http_status: Some(400),
                        source: sentinel(),
                    },
                ),
                ("identity_removal", "cognito", "authentication", Some(400)),
            ),
            (
                DeleteUserIdentityError::Completion(
                    UserIdentityDeletionError::InvalidPersistedState(sentinel()),
                ),
                ("completion", "postgres", "invalid_persisted_state", None),
            ),
        ];
        for (error, (stage, dependency, category, http_status)) in cases {
            let handler = failing(error);
            let job = claimed_job(JOB);
            let (logs, guard) = CapturedLogs::start();

            let disposition = handler.handle(&job).await;
            drop(guard);

            assert_eq!(
                Disposition::Retry(error_class("deletion_unconfirmed")),
                disposition,
                "{stage}"
            );
            let events = logs.events_where("event", "user.identity_deletion.retained");
            assert_eq!(1, events.len(), "{stage}");
            assert_eq!(
                Some(job.id().to_string().as_str()),
                events[0]["job_id"].as_str()
            );
            assert_eq!(Some("deletion_unconfirmed"), events[0]["outcome"].as_str());
            assert_eq!(Some(stage), events[0]["stage"].as_str());
            assert_eq!(Some(dependency), events[0]["dependency"].as_str());
            assert_eq!(Some(category), events[0]["error_category"].as_str());
            assert_eq!(
                http_status,
                events[0]
                    .get("http_status")
                    .and_then(|status| status.as_u64())
            );
            assert!(logs.text().contains(r#""level":"WARN""#), "{stage}");
            assert!(!logs.text().contains("SENTINEL"), "{stage}");
        }
    }

    #[tokio::test]
    async fn should_retain_an_invalid_job_with_a_warning_and_no_failure_facts() {
        let (logs, guard) = CapturedLogs::start();

        let disposition = UserIdentityDeletionJobs::new(stub(0))
            .handle(&claimed_job("bad body"))
            .await;
        drop(guard);

        assert_eq!(Disposition::Retry(error_class("invalid_job")), disposition);
        let retained = logs.events_where("event", "user.identity_deletion.retained");
        assert_eq!(1, retained.len());
        assert_eq!(Some("invalid_job"), retained[0]["outcome"].as_str());
        assert!(retained[0].get("stage").is_none());
        assert!(logs.text().contains(r#""level":"WARN""#));
    }

    #[tokio::test]
    async fn should_retain_an_actively_leased_deletion_without_a_failure_warning() {
        let (logs, guard) = CapturedLogs::start();

        let disposition = UserIdentityDeletionJobs::new(stub(1))
            .handle(&claimed_job(JOB))
            .await;
        drop(guard);

        assert_eq!(Disposition::Retry(error_class("active_lease")), disposition);
        let retained = logs.events_where("event", "user.identity_deletion.retained");
        assert_eq!(1, retained.len());
        assert_eq!(Some("active_lease"), retained[0]["outcome"].as_str());
        assert!(
            logs.text()
                .lines()
                .all(|line| !line.contains(r#""level":"WARN""#))
        );
    }

    #[tokio::test]
    async fn should_retain_contended_deletions_without_a_failure_warning() {
        let cases = [
            DeleteUserIdentityError::TargetSave(UserIdentityDeletionError::ClaimLost),
            DeleteUserIdentityError::Completion(UserIdentityDeletionError::ClaimLost),
        ];
        for error in cases {
            let handler = failing(error);
            let (logs, guard) = CapturedLogs::start();

            let disposition = handler.handle(&claimed_job(JOB)).await;
            drop(guard);

            assert_eq!(Disposition::Retry(error_class("claim_lost")), disposition);
            let retained = logs.events_where("event", "user.identity_deletion.retained");
            assert_eq!(1, retained.len());
            assert_eq!(Some("claim_lost"), retained[0]["outcome"].as_str());
            assert!(retained[0].get("stage").is_none());
            assert!(
                logs.text()
                    .lines()
                    .all(|line| !line.contains(r#""level":"WARN""#))
            );
        }
    }
}
