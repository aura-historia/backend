//! Database coordination shared by transaction-bound, context-owned receipt adapters.
use application::error::box_error;
use application::idempotency::{CommandFingerprint, CommandReceiptError};
use sqlx::PgConnection;

/// Never wait on a duplicate while holding other aggregate locks. Hash collisions only
/// cause a retryable busy result; receipt uniqueness and intent comparison remain authoritative.
pub async fn coordinate_command(
    connection: &mut PgConnection,
    identity: &[u8],
) -> Result<(), CommandReceiptError> {
    let digest = CommandFingerprint::digest(1, identity);
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&digest.as_bytes()[..8]);
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(i64::from_be_bytes(bytes))
        .fetch_one(connection)
        .await
        .map_err(|source| CommandReceiptError::Storage(box_error(source)))?;
    if acquired {
        Ok(())
    } else {
        Err(CommandReceiptError::InProgress)
    }
}
