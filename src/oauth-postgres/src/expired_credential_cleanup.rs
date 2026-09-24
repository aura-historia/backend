use application::error::box_error;
use oauth_service::ports::{
    CleanupExpiredCredentialsAndProviderReceipts, ExpiredCredentialCleanupError,
};
use sqlx::PgPool;

#[derive(Clone)]
pub struct SqlxExpiredCredentialCleanup {
    pool: PgPool,
}

impl SqlxExpiredCredentialCleanup {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl CleanupExpiredCredentialsAndProviderReceipts for SqlxExpiredCredentialCleanup {
    async fn cleanup_expired_credentials_and_provider_receipts(
        &self,
        batch_size: i32,
    ) -> Result<(i64, i64, i64, i64), ExpiredCredentialCleanupError> {
        sqlx::query_as("SELECT * FROM cleanup_expired_credentials_and_provider_receipts($1)")
            .bind(batch_size)
            .fetch_one(&self.pool)
            .await
            .map_err(
                |source| ExpiredCredentialCleanupError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )
    }
}
