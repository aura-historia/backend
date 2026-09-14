use crate::mapping::{AuctionRow, map_error, map_stored_auction};
use application::error::box_error;
use auction_core::AuctionId;
use auction_service::ports::{
    AuctionSummary, AuctionSummaryBatchReadError, AuctionSummaryBatchReader,
};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub struct SqlxAuctionSummaryBatchReader {
    pool: PgPool,
}

impl SqlxAuctionSummaryBatchReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl AuctionSummaryBatchReader for SqlxAuctionSummaryBatchReader {
    async fn find_summaries(
        &self,
        auction_ids: &[AuctionId],
    ) -> Result<HashMap<AuctionId, AuctionSummary>, AuctionSummaryBatchReadError> {
        let auction_ids = auction_ids
            .iter()
            .map(|id| id.as_uuid())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if auction_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut connection = self.pool.acquire().await.map_err(|error| {
            AuctionSummaryBatchReadError::QueryFailed {
                source: box_error(error),
            }
        })?;
        let rows = sqlx::query_as::<_, AuctionRow>(
            "SELECT auction_id, listing_source_id, source_auction_id, name_text, name_language, description_text, description_language, catalogue_url, format, bidding_opens_at, live_starts_at, lots_begin_closing_at, scheduled_end_at, reported_status, reported_lot_count, version, created, updated FROM auctions WHERE auction_id = ANY($1)",
        )
        .bind(&auction_ids)
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| AuctionSummaryBatchReadError::QueryFailed {
            source: box_error(error),
        })?;

        rows.into_iter()
            .map(|row| {
                let stored = map_stored_auction(row).map_err(|error| {
                    AuctionSummaryBatchReadError::InvalidReadModel {
                        source: map_error(error),
                    }
                })?;
                let auction = stored.auction;
                Ok((
                    auction.id(),
                    AuctionSummary {
                        auction_id: auction.id(),
                        name: auction.name().cloned(),
                        format: auction.format(),
                        reported_status: auction.reported_status(),
                        schedule: auction.schedule().clone(),
                    },
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auction_service::ports::AuctionSummaryBatchReader;
    use listing_source_core::ListingSourceId;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use time::macros::datetime;

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    async fn source(pool: &PgPool) -> ListingSourceId {
        let source_id = ListingSourceId::new();
        let party_id = uuid::Uuid::now_v7();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(party_id)
            .bind(format!("party-{party_id}"))
            .bind("Auction source")
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert party: {error}"));
        sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
            .bind(source_id.as_uuid())
            .bind(format!("source-{}", source_id.as_uuid()))
            .bind("Auction source")
            .bind(party_id)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert listing source: {error}"));
        source_id
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_batch_unique_auction_ids_and_reconstruct_flat_schedule() {
        let pool = get_postgres_client().await;
        let source_id = source(&pool).await;
        let auction_id = AuctionId::new();
        sqlx::query("INSERT INTO auctions (auction_id, listing_source_id, source_auction_id, format, reported_status, bidding_opens_at, scheduled_end_at) VALUES ($1, $2, $3, $4, $5, $6, $7)")
            .bind(auction_id.as_uuid())
            .bind(source_id.as_uuid())
            .bind("catalogue-42")
            .bind("TIMED")
            .bind("SCHEDULED")
            .bind(datetime!(2026-10-18 16:00 UTC))
            .bind(datetime!(2026-10-19 16:00 UTC))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert auction: {error}"));

        let summaries = SqlxAuctionSummaryBatchReader::new(pool)
            .find_summaries(&[auction_id, auction_id, AuctionId::new()])
            .await
            .unwrap_or_else(|error| panic!("read summaries: {error}"));

        assert_eq!(1, summaries.len());
        let summary = summaries
            .get(&auction_id)
            .unwrap_or_else(|| panic!("summary must exist"));
        assert_eq!(Some(auction_core::AuctionFormat::Timed), summary.format);
        assert_eq!(
            Some(auction_core::AuctionReportedStatus::Scheduled),
            summary.reported_status
        );
        assert_eq!(
            Some(datetime!(2026-10-18 16:00 UTC)),
            summary.schedule.bidding_opens()
        );
        assert_eq!(
            Some(datetime!(2026-10-19 16:00 UTC)),
            summary.schedule.scheduled_end()
        );
    }
}
