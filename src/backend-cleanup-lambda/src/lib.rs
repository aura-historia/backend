use lambda_runtime::Error;
use oauth_service::use_cases::{
    CleanupExpiredCredentialsAndProviderReceiptsUseCase, ExpiryCleanupBatchSize,
    ExpiryCleanupBatchSizeError,
};
use std::env;
use std::time::Instant;

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
    batch_size: ExpiryCleanupBatchSize,
) -> Result<(), Error> {
    let started_at = Instant::now();
    match cleanup.execute(batch_size).await {
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
            Ok(())
        }
        Err(_) => {
            tracing::warn!(
                event = "backend_cleanup.expired_credentials.completed",
                outcome = "failure",
                batch_size = batch_size.get(),
                duration_ms = started_at.elapsed().as_millis(),
            );
            Err(Error::from("expired credential cleanup failed"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth_service::ports::ExpiredCredentialCleanupError;
    use oauth_service::use_cases::CleanupCounts;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct RecordingCleanup {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl CleanupExpiredCredentialsAndProviderReceiptsUseCase for RecordingCleanup {
        async fn execute(
            &self,
            _batch_size: ExpiryCleanupBatchSize,
        ) -> Result<CleanupCounts, ExpiredCredentialCleanupError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(CleanupCounts {
                access_tokens_deleted: 1,
                authorization_codes_deleted: 2,
                third_party_exchange_codes_deleted: 3,
                provider_receipts_deleted: 4,
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
    async fn should_invoke_expiry_cleanup_once_per_lambda_invocation() {
        let cleanup = RecordingCleanup {
            calls: AtomicUsize::new(0),
        };
        let batch_size = ExpiryCleanupBatchSize::try_from(100).unwrap();

        run_cleanup(&cleanup, batch_size).await.unwrap();

        assert_eq!(cleanup.calls.load(Ordering::Relaxed), 1);
    }
}
