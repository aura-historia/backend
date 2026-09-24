use crate::ports::{CleanupExpiredCredentialsAndProviderReceipts, ExpiredCredentialCleanupError};

const MIN_BATCH_SIZE: i32 = 1;
const MAX_BATCH_SIZE: i32 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpiryCleanupBatchSize(i32);

impl ExpiryCleanupBatchSize {
    pub fn get(self) -> i32 {
        self.0
    }
}

impl TryFrom<i32> for ExpiryCleanupBatchSize {
    type Error = ExpiryCleanupBatchSizeError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        if !(MIN_BATCH_SIZE..=MAX_BATCH_SIZE).contains(&value) {
            return Err(ExpiryCleanupBatchSizeError::OutOfRange { value });
        }
        Ok(Self(value))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExpiryCleanupBatchSizeError {
    #[error("expiry cleanup batch size must be between 1 and 1000")]
    OutOfRange { value: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupCounts {
    pub access_tokens_deleted: i64,
    pub authorization_codes_deleted: i64,
    pub third_party_exchange_codes_deleted: i64,
    pub provider_receipts_deleted: i64,
}

#[async_trait::async_trait]
pub trait CleanupExpiredCredentialsAndProviderReceiptsUseCase: Send + Sync {
    async fn execute(
        &self,
        batch_size: ExpiryCleanupBatchSize,
    ) -> Result<CleanupCounts, ExpiredCredentialCleanupError>;
}

pub struct CleanupExpiredCredentialsAndProviderReceiptsHandler<C> {
    cleanup: C,
}

impl<C> CleanupExpiredCredentialsAndProviderReceiptsHandler<C> {
    pub fn new(cleanup: C) -> Self {
        Self { cleanup }
    }
}

#[async_trait::async_trait]
impl<C> CleanupExpiredCredentialsAndProviderReceiptsUseCase
    for CleanupExpiredCredentialsAndProviderReceiptsHandler<C>
where
    C: CleanupExpiredCredentialsAndProviderReceipts,
{
    async fn execute(
        &self,
        batch_size: ExpiryCleanupBatchSize,
    ) -> Result<CleanupCounts, ExpiredCredentialCleanupError> {
        let (
            access_tokens_deleted,
            authorization_codes_deleted,
            third_party_exchange_codes_deleted,
            provider_receipts_deleted,
        ) = self
            .cleanup
            .cleanup_expired_credentials_and_provider_receipts(batch_size.get())
            .await?;

        Ok(CleanupCounts {
            access_tokens_deleted,
            authorization_codes_deleted,
            third_party_exchange_codes_deleted,
            provider_receipts_deleted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_cleanup_batch_sizes_outside_the_database_contract() {
        assert_eq!(
            Err(ExpiryCleanupBatchSizeError::OutOfRange { value: 0 }),
            ExpiryCleanupBatchSize::try_from(0),
        );
        assert_eq!(
            Err(ExpiryCleanupBatchSizeError::OutOfRange { value: 1_001 }),
            ExpiryCleanupBatchSize::try_from(1_001),
        );
        assert_eq!(
            Ok(1_000),
            ExpiryCleanupBatchSize::try_from(1_000).map(ExpiryCleanupBatchSize::get)
        );
    }
}
