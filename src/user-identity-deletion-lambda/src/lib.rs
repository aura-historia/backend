use aura_historia_jobs::{DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_sqs::{RecordOutcome, process_batch};
use std::time::Duration;
use user_service::use_cases::commands::delete_user_identity::{
    DeleteUserIdentityResult, DeleteUserIdentityUseCase,
};

enum JobOutcome {
    Completed,
    Deferred,
    Retry,
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
        Ok(DeleteUserIdentityResult::Deferred) => JobOutcome::Deferred,
        Err(_) => JobOutcome::Retry,
    }
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
        let category = match attempt.outcome {
            RecordOutcome::Completed(JobOutcome::Completed) => return true,
            RecordOutcome::Completed(JobOutcome::Deferred) => "active_lease",
            RecordOutcome::Completed(JobOutcome::Retry) => "deletion_unconfirmed",
            RecordOutcome::Completed(JobOutcome::Invalid) => "invalid_job",
            RecordOutcome::MissingBody => "missing_body",
            RecordOutcome::InsufficientBudget => "insufficient_budget",
            RecordOutcome::TimedOut => "timed_out",
            RecordOutcome::Panicked => "panicked",
        };
        tracing::warn!(
            event = "user.identity_deletion.retained",
            message_id = %attempt.message_id,
            outcome = category,
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
    use user_core::user_id::UserId;
    use user_service::ports::user_identity_deletion::UserIdentityDeletionError;
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
        ) -> Result<DeleteUserIdentityResult, UserIdentityDeletionError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.outcome {
                0 => Ok(DeleteUserIdentityResult::Completed),
                1 => Ok(DeleteUserIdentityResult::Deferred),
                2 => Err(UserIdentityDeletionError(application::error::static_error(
                    "provider unavailable",
                ))),
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
}
