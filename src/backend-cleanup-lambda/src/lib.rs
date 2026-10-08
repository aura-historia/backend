use lambda_runtime::Error;
use oauth_service::use_cases::{
    CleanupExpiredCredentialsAndProviderReceiptsUseCase, ExpiryCleanupBatchSize,
    ExpiryCleanupBatchSizeError,
};
use std::env;
use std::time::Instant;
use user_service::use_cases::CleanupConsentWorkflowUseCase;

pub const EXPIRY_CLEANUP_BATCH_SIZE_ENV: &str = "EXPIRY_CLEANUP_BATCH_SIZE";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CleanupConfigError {
    #[error("missing required cleanup configuration {name}")]
    Missing { name: &'static str },
    #[error("invalid cleanup configuration {name}")]
    Invalid { name: &'static str },
}

pub fn cleanup_batch_size_from_env() -> Result<ExpiryCleanupBatchSize, CleanupConfigError> {
    cleanup_batch_size_from_result(env::var(EXPIRY_CLEANUP_BATCH_SIZE_ENV))
}

fn cleanup_batch_size_from_result(
    value: Result<String, env::VarError>,
) -> Result<ExpiryCleanupBatchSize, CleanupConfigError> {
    let value = match value {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) | Err(env::VarError::NotPresent) => {
            return Err(CleanupConfigError::Missing {
                name: EXPIRY_CLEANUP_BATCH_SIZE_ENV,
            });
        }
        Err(env::VarError::NotUnicode(_)) => {
            return Err(CleanupConfigError::Invalid {
                name: EXPIRY_CLEANUP_BATCH_SIZE_ENV,
            });
        }
    };
    let value = value
        .parse::<i32>()
        .map_err(|_| CleanupConfigError::Invalid {
            name: EXPIRY_CLEANUP_BATCH_SIZE_ENV,
        })?;
    ExpiryCleanupBatchSize::try_from(value).map_err(|_: ExpiryCleanupBatchSizeError| {
        CleanupConfigError::Invalid {
            name: EXPIRY_CLEANUP_BATCH_SIZE_ENV,
        }
    })
}

pub async fn run_cleanup(
    cleanup: &(dyn CleanupExpiredCredentialsAndProviderReceiptsUseCase + Send + Sync),
    consent_cleanup: &(dyn CleanupConsentWorkflowUseCase + Send + Sync),
    batch_size: ExpiryCleanupBatchSize,
) -> Result<(), Error> {
    let started_at = Instant::now();
    let credential_result = cleanup.execute(batch_size).await;
    match &credential_result {
        Ok(counts) => {
            tracing::info!(
                event = "backend_cleanup.expired_credentials.completed",
                outcome = "success",
                batch_size = batch_size.get(),
                access_tokens_deleted = counts.access_tokens_deleted,
                authorization_codes_deleted = counts.authorization_codes_deleted,
                third_party_exchange_codes_deleted = counts.third_party_exchange_codes_deleted,
                provider_receipts_deleted = counts.provider_receipts_deleted,
                duration_ms = started_at.elapsed().as_millis(),
            );
        }
        Err(_) => {
            tracing::warn!(
                event = "backend_cleanup.expired_credentials.completed",
                outcome = "failure",
                batch_size = batch_size.get(),
                duration_ms = started_at.elapsed().as_millis(),
            );
        }
    }
    let consent_result = consent_cleanup.execute(batch_size.get() as u16).await;
    match &consent_result {
        Ok(counts) => tracing::info!(
            event = "backend_cleanup.consent_workflow.completed",
            outcome = "success",
            batch_size = batch_size.get(),
            unconfirmed_challenges_deleted = counts.unconfirmed_challenges_deleted,
            confirmed_challenges_deleted = counts.confirmed_challenges_deleted,
            completed_intents_deleted = counts.completed_intents_deleted,
            webhook_receipts_deleted = counts.webhook_receipts_deleted,
            duration_ms = started_at.elapsed().as_millis(),
        ),
        Err(_) => tracing::warn!(
            event = "backend_cleanup.consent_workflow.completed",
            outcome = "failure",
            batch_size = batch_size.get(),
            duration_ms = started_at.elapsed().as_millis(),
        ),
    }
    if credential_result.is_err() || consent_result.is_err() {
        Err(Error::from("backend cleanup failed"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth_service::ports::ExpiredCredentialCleanupError;
    use oauth_service::use_cases::CleanupCounts;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use user_service::use_cases::{CleanupConsentWorkflowError, ConsentWorkflowCleanupCounts};

    struct RecordingCleanup {
        calls: AtomicUsize,
        fail: bool,
    }

    struct RecordingConsentCleanup {
        calls: AtomicUsize,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl CleanupExpiredCredentialsAndProviderReceiptsUseCase for RecordingCleanup {
        async fn execute(
            &self,
            _batch_size: ExpiryCleanupBatchSize,
        ) -> Result<CleanupCounts, ExpiredCredentialCleanupError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                return Err(ExpiredCredentialCleanupError::TemporarilyUnavailable {
                    source: Box::new(std::io::Error::other("fixture failure")),
                });
            }
            Ok(CleanupCounts {
                access_tokens_deleted: 1,
                authorization_codes_deleted: 2,
                third_party_exchange_codes_deleted: 3,
                provider_receipts_deleted: 4,
            })
        }
    }

    #[async_trait::async_trait]
    impl CleanupConsentWorkflowUseCase for RecordingConsentCleanup {
        async fn execute(
            &self,
            _batch_size: u16,
        ) -> Result<ConsentWorkflowCleanupCounts, CleanupConsentWorkflowError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                return Err(CleanupConsentWorkflowError::TemporarilyUnavailable);
            }
            Ok(ConsentWorkflowCleanupCounts {
                unconfirmed_challenges_deleted: 1,
                confirmed_challenges_deleted: 2,
                completed_intents_deleted: 3,
                webhook_receipts_deleted: 4,
            })
        }
    }

    #[test]
    fn should_fail_closed_for_missing_or_invalid_cleanup_batch_size() {
        for value in [
            Err(env::VarError::NotPresent),
            Ok(String::new()),
            Ok("0".to_owned()),
            Ok("1001".to_owned()),
            Ok("invalid".to_owned()),
        ] {
            assert!(cleanup_batch_size_from_result(value).is_err());
        }
    }

    #[test]
    fn should_accept_a_valid_cleanup_batch_size() {
        assert_eq!(
            cleanup_batch_size_from_result(Ok("100".to_owned())).map(ExpiryCleanupBatchSize::get),
            Ok(100),
        );
    }

    #[tokio::test]
    async fn should_invoke_each_cleanup_once_per_lambda_invocation() {
        let cleanup = RecordingCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let consent = RecordingConsentCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let batch_size = ExpiryCleanupBatchSize::try_from(100).unwrap();

        run_cleanup(&cleanup, &consent, batch_size).await.unwrap();

        assert_eq!(cleanup.calls.load(Ordering::Relaxed), 1);
        assert_eq!(consent.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn consent_failure_fails_invocation_after_existing_cleanup() {
        let cleanup = RecordingCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let consent = RecordingConsentCleanup {
            calls: AtomicUsize::new(0),
            fail: true,
        };
        let batch_size = ExpiryCleanupBatchSize::try_from(100).unwrap();

        assert!(run_cleanup(&cleanup, &consent, batch_size).await.is_err());
        assert_eq!(cleanup.calls.load(Ordering::Relaxed), 1);
        assert_eq!(consent.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn existing_cleanup_failure_still_invokes_consent_cleanup_and_fails() {
        let cleanup = RecordingCleanup {
            calls: AtomicUsize::new(0),
            fail: true,
        };
        let consent = RecordingConsentCleanup {
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let batch_size = ExpiryCleanupBatchSize::try_from(100).unwrap();

        assert!(run_cleanup(&cleanup, &consent, batch_size).await.is_err());
        assert_eq!(cleanup.calls.load(Ordering::Relaxed), 1);
        assert_eq!(consent.calls.load(Ordering::Relaxed), 1);
    }
}
