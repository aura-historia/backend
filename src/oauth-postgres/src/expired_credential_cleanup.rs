use application::error::box_error;
use oauth_service::ports::{
    CleanupCounts, CleanupExpiredCredentialsAndProviderReceipts, ExpiredCredentialCleanupError,
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
    ) -> Result<CleanupCounts, ExpiredCredentialCleanupError> {
        let row: CleanupCountsRow = sqlx::query_as(
            "SELECT access_tokens_deleted, authorization_codes_deleted, \
                    third_party_exchange_codes_deleted, provider_receipts_deleted, \
                    refresh_tokens_deleted, refresh_token_families_deleted, \
                    oauth_connections_deleted \
             FROM cleanup_expired_credentials_and_provider_receipts($1)",
        )
        .bind(batch_size)
        .fetch_one(&self.pool)
        .await
        .map_err(
            |source| ExpiredCredentialCleanupError::TemporarilyUnavailable {
                source: box_error(source),
            },
        )?;
        Ok(CleanupCounts {
            access_tokens_deleted: row.access_tokens_deleted,
            authorization_codes_deleted: row.authorization_codes_deleted,
            third_party_exchange_codes_deleted: row.third_party_exchange_codes_deleted,
            provider_receipts_deleted: row.provider_receipts_deleted,
            refresh_tokens_deleted: row.refresh_tokens_deleted,
            refresh_token_families_deleted: row.refresh_token_families_deleted,
            oauth_connections_deleted: row.oauth_connections_deleted,
        })
    }
}

#[derive(sqlx::FromRow)]
struct CleanupCountsRow {
    access_tokens_deleted: i64,
    authorization_codes_deleted: i64,
    third_party_exchange_codes_deleted: i64,
    provider_receipts_deleted: i64,
    refresh_tokens_deleted: i64,
    refresh_token_families_deleted: i64,
    oauth_connections_deleted: i64,
}
