use crate::{mapping::AuctionRow, repositories::auction_repository::load};
use application::error::box_error;
use auction_service::ports::{AuctionDetails, AuctionDetailsReadError, AuctionDetailsReader};
use sqlx::PgPool;

#[derive(Clone)]
pub struct SqlxAuctionDetailsReader {
    pool: PgPool,
}

impl SqlxAuctionDetailsReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl AuctionDetailsReader for SqlxAuctionDetailsReader {
    async fn find_by_id(
        &self,
        id: auction_core::AuctionId,
    ) -> Result<Option<AuctionDetails>, AuctionDetailsReadError> {
        let mut connection = self.pool.acquire().await.map_err(|error| {
            AuctionDetailsReadError::TemporarilyUnavailable {
                source: box_error(error),
            }
        })?;
        let row = sqlx::query_as::<_, AuctionRow>("SELECT auction_id, listing_source_id, source_auction_id, name_text, name_language, description_text, description_language, catalogue_url, format, bidding_opens_at, live_starts_at, lots_begin_closing_at, scheduled_end_at, reported_status, reported_lot_count, version, created, updated FROM auctions WHERE auction_id=$1")
        .bind(id.as_uuid())
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| AuctionDetailsReadError::TemporarilyUnavailable {
            source: box_error(error),
        })?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored = load(row).map_err(|error| match error {
            auction_service::ports::AuctionRepositoryError::InvalidPersistedState { source } => {
                AuctionDetailsReadError::InvalidPersistedState { source }
            }
            auction_service::ports::AuctionRepositoryError::TemporarilyUnavailable { source } => {
                AuctionDetailsReadError::TemporarilyUnavailable { source }
            }
            auction_service::ports::AuctionRepositoryError::ConcurrencyConflict => {
                AuctionDetailsReadError::Internal {
                    source: box_error(std::io::Error::other(
                        "unexpected auction read concurrency conflict",
                    )),
                }
            }
            auction_service::ports::AuctionRepositoryError::SourceAuctionAlreadyExists {
                source,
            }
            | auction_service::ports::AuctionRepositoryError::ListingSourceNotFound { source }
            | auction_service::ports::AuctionRepositoryError::Internal { source } => {
                AuctionDetailsReadError::Internal { source }
            }
        })?;
        Ok(Some(AuctionDetails { stored }))
    }
}
