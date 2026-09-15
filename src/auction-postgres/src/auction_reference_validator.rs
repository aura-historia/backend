use application::error::box_error;
use auction_service::ports::{
    AuctionReferenceValidationError, AuctionReferenceValidator, AuctionReferenceValidatorFactory,
};
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxAuctionReferenceValidatorFactory;

struct SqlxAuctionReferenceValidator<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxAuctionReferenceValidatorFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AuctionReferenceValidatorFactory<SqlxTransaction> for SqlxAuctionReferenceValidatorFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl AuctionReferenceValidator + 'tx {
        SqlxAuctionReferenceValidator {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl AuctionReferenceValidator for SqlxAuctionReferenceValidator<'_> {
    async fn validate(
        &mut self,
        auction_id: auction_core::AuctionId,
        listing_source_id: listing_source_core::ListingSourceId,
    ) -> Result<(), AuctionReferenceValidationError> {
        let source = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT listing_source_id FROM auctions WHERE auction_id = $1 FOR KEY SHARE",
        )
        .bind(auction_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(
            |source| AuctionReferenceValidationError::TemporarilyUnavailable {
                source: box_error(source),
            },
        )?
        .ok_or(AuctionReferenceValidationError::NotFound)?;
        if source != *listing_source_id.as_uuid() {
            return Err(AuctionReferenceValidationError::ListingSourceMismatch);
        }
        Ok(())
    }
}
