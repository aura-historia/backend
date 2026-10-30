use aura_historia_jobs::{DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_bootstrap::{Dependency, FailureDiagnostic, INVALID_PERSISTED_STATE};
use platform_lambda_sqs::{RecordOutcome, process_batch};
use std::time::Duration;
use user_service::ports::user_identity_deletion::UserIdentityDeletionError;
use user_service::use_cases::commands::delete_user_identity::{
    DeleteUserIdentityError, DeleteUserIdentityResult, DeleteUserIdentityUseCase,
};

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

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &dyn DeleteUserIdentityUseCase,
    budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(event, budget, Duration::from_secs(25), |body| async move {
        process_job(&body, use_case).await
    })
    .await?;
    Ok(results.finish(|attempt| {
        let (category, failure) = match &attempt.outcome {
            RecordOutcome::Completed(JobOutcome::Completed) => return true,
            RecordOutcome::Completed(JobOutcome::Contended(category)) => {
                // Expected contention, not a failure: the other attempt completes the deletion.
                tracing::info!(
                    event = "user.identity_deletion.retained",
                    message_id = %attempt.message_id,
                    outcome = *category,
                    "User identity deletion retained while another attempt owns it",
                );
                return false;
            }
            RecordOutcome::Completed(JobOutcome::Retry(failure)) => {
                ("deletion_unconfirmed", Some(failure))
            }
            RecordOutcome::Completed(JobOutcome::Invalid) => ("invalid_job", None),
            RecordOutcome::MissingBody => ("missing_body", None),
            RecordOutcome::InsufficientBudget => ("insufficient_budget", None),
            RecordOutcome::TimedOut => ("timed_out", None),
            RecordOutcome::Panicked => ("panicked", None),
        };
        tracing::warn!(
            event = "user.identity_deletion.retained",
            message_id = %attempt.message_id,
            outcome = category,
            stage = failure.map(FailureDiagnostic::stage),
            dependency = failure.and_then(FailureDiagnostic::dependency),
            error_category = failure.map(FailureDiagnostic::category),
            http_status = failure.and_then(FailureDiagnostic::http_status),
            sqlstate = failure.and_then(FailureDiagnostic::sqlstate),
            "User identity deletion retained for SQS retry or redrive",
        );
        false
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::sqs::SqsMessage;
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
                2 => Err(DeleteUserIdentityError::IdentityRemoval(
                    UserIdentityDeletionError::IdentityProvider {
                        failure: IdentityProviderFailure::Unavailable,
                        http_status: Some(503),
                        source: application::error::static_error("provider unavailable"),
                    },
                )),
                3 => std::future::pending().await,
                _ => panic!("worker panic"),
            }
        }
    }
    fn event(body: Option<String>) -> LambdaEvent<SqsEvent> {
        let mut message = SqsMessage::default();
        message.message_id = Some("message-1".into());
        message.body = body;
        let mut event = SqsEvent::default();
        event.records.push(message);
        LambdaEvent::new(event, lambda_runtime::Context::default())
    }
    #[tokio::test]
    async fn only_committed_completion_acknowledges_the_message() {
        for outcome in [0, 1, 2, 4] {
            let stub = Stub {
                outcome,
                calls: AtomicUsize::new(0),
            };
            let response = handler(event(Some(JOB.into())), &stub, Duration::from_secs(30))
                .await
                .unwrap();
            assert_eq!(
                usize::from(outcome != 0),
                response.batch_item_failures.len()
            );
            assert_eq!(1, stub.calls.load(Ordering::SeqCst));
        }
        let pending = Stub {
            outcome: 3,
            calls: AtomicUsize::new(0),
        };
        let response = handler(event(Some(JOB.into())), &pending, Duration::from_millis(1))
            .await
            .unwrap();
        assert_eq!(1, response.batch_item_failures.len());
    }
    #[tokio::test]
    async fn invalid_jobs_missing_bodies_and_wrong_scope_never_call_the_use_case() {
        let stub = Stub {
            outcome: 0,
            calls: AtomicUsize::new(0),
        };
        for body in [
            None,
            Some("bad body".into()),
            Some(JOB.replace("usr_", "mci_")),
            Some(JOB.replace(
                "\"scope\":\"user-identity-deletion\"",
                "\"scope\":\"notification-delivery\"",
            )),
        ] {
            let response = handler(event(body), &stub, Duration::from_secs(30))
                .await
                .unwrap();
            assert_eq!(1, response.batch_item_failures.len());
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
            let use_case = Failing(std::sync::Mutex::new(Some(error)));
            let (logs, guard) = CapturedLogs::start();

            let response = handler(event(Some(JOB.into())), &use_case, Duration::from_secs(30))
                .await
                .unwrap();
            drop(guard);

            assert_eq!(1, response.batch_item_failures.len());
            let events = logs.events_where("event", "user.identity_deletion.retained");
            assert_eq!(1, events.len(), "{stage}");
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
    async fn should_retain_an_actively_leased_deletion_without_a_failure_warning() {
        let stub = Stub {
            outcome: 1,
            calls: AtomicUsize::new(0),
        };
        let (logs, guard) = CapturedLogs::start();

        let response = handler(event(Some(JOB.into())), &stub, Duration::from_secs(30))
            .await
            .unwrap();
        drop(guard);

        assert_eq!(1, response.batch_item_failures.len());
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
            (
                DeleteUserIdentityError::TargetSave(UserIdentityDeletionError::ClaimLost),
                "claim_lost",
            ),
            (
                DeleteUserIdentityError::Completion(UserIdentityDeletionError::ClaimLost),
                "claim_lost",
            ),
        ];
        for (error, outcome) in cases {
            let use_case = Failing(std::sync::Mutex::new(Some(error)));
            let (logs, guard) = CapturedLogs::start();

            let response = handler(event(Some(JOB.into())), &use_case, Duration::from_secs(30))
                .await
                .unwrap();
            drop(guard);

            assert_eq!(1, response.batch_item_failures.len());
            let retained = logs.events_where("event", "user.identity_deletion.retained");
            assert_eq!(1, retained.len());
            assert_eq!(Some(outcome), retained[0]["outcome"].as_str());
            assert!(retained[0].get("stage").is_none());
            assert!(
                logs.text()
                    .lines()
                    .all(|line| !line.contains(r#""level":"WARN""#))
            );
        }
    }
}
