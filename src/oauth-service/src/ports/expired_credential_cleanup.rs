use application::error::BoxError;

/// Rows one bounded cleanup call deleted, per target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupCounts {
    pub access_tokens_deleted: i64,
    pub authorization_codes_deleted: i64,
    pub third_party_exchange_codes_deleted: i64,
    pub provider_receipts_deleted: i64,
    pub refresh_tokens_deleted: i64,
    pub refresh_token_families_deleted: i64,
    pub oauth_connections_deleted: i64,
}

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
    ) -> Result<CleanupCounts, ExpiredCredentialCleanupError>;
}
