use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum ExpiredCredentialCleanupError {
    #[error("expired credential cleanup is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait CleanupExpiredCredentialsAndProviderReceipts: Send + Sync {
    async fn cleanup_expired_credentials_and_provider_receipts(
        &self,
        batch_size: i32,
    ) -> Result<(i64, i64, i64, i64), ExpiredCredentialCleanupError>;
}
