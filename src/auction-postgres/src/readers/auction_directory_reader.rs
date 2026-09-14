use crate::mapping::{AuctionRow, AuctionSchedulePointRow, map_error, map_stored_auction};
use application::{
    error::{BoxError, box_error},
    pagination::{Cursor, CursoredResult},
};
use auction_core::AuctionSchedulePoint;
use auction_service::ports::{
    AuctionDirectoryCursor, AuctionDirectoryReadError, AuctionDirectoryReader,
    ListAuctionsDirectoryRequest, ListAuctionsDirectoryResult, PublicAuctionDirectoryItem,
    PublicAuctionDirectorySourceSummary,
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use sqlx::{PgPool, Postgres, QueryBuilder};
use std::collections::HashMap;

#[derive(Clone)]
pub struct SqlxAuctionDirectoryReader {
    pool: PgPool,
}

impl SqlxAuctionDirectoryReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct AuctionDirectoryRow {
    auction_id: uuid::Uuid,
    listing_source_id: uuid::Uuid,
    source_auction_id: String,
    name_text: Option<String>,
    name_language: Option<String>,
    description_text: Option<String>,
    description_language: Option<String>,
    catalogue_url: Option<String>,
    format: Option<String>,
    reported_status: Option<String>,
    reported_lot_count: Option<i64>,
    version: i64,
    created: time::OffsetDateTime,
    updated: time::OffsetDateTime,
    listing_source_slug_id: String,
    listing_source_name: String,
}

#[derive(Debug, sqlx::FromRow)]
struct JoinedAuctionDirectoryRow {
    auction_id: uuid::Uuid,
    listing_source_id: uuid::Uuid,
    source_auction_id: String,
    name_text: Option<String>,
    name_language: Option<String>,
    description_text: Option<String>,
    description_language: Option<String>,
    catalogue_url: Option<String>,
    format: Option<String>,
    reported_status: Option<String>,
    reported_lot_count: Option<i64>,
    version: i64,
    created: time::OffsetDateTime,
    updated: time::OffsetDateTime,
    listing_source_slug_id: String,
    listing_source_name: String,
    role: Option<String>,
    precision: Option<String>,
    instant_at: Option<time::OffsetDateTime>,
    date_on: Option<time::Date>,
    source_timezone: Option<String>,
}

impl JoinedAuctionDirectoryRow {
    fn auction(&self) -> AuctionDirectoryRow {
        AuctionDirectoryRow {
            auction_id: self.auction_id,
            listing_source_id: self.listing_source_id,
            source_auction_id: self.source_auction_id.clone(),
            name_text: self.name_text.clone(),
            name_language: self.name_language.clone(),
            description_text: self.description_text.clone(),
            description_language: self.description_language.clone(),
            catalogue_url: self.catalogue_url.clone(),
            format: self.format.clone(),
            reported_status: self.reported_status.clone(),
            reported_lot_count: self.reported_lot_count,
            version: self.version,
            created: self.created,
            updated: self.updated,
            listing_source_slug_id: self.listing_source_slug_id.clone(),
            listing_source_name: self.listing_source_name.clone(),
        }
    }

    fn schedule_point(&self) -> Option<AuctionSchedulePointRow> {
        match (&self.role, &self.precision) {
            (Some(role), Some(precision)) => Some(AuctionSchedulePointRow {
                role: role.clone(),
                precision: precision.clone(),
                instant_at: self.instant_at,
                date_on: self.date_on,
                source_timezone: self.source_timezone.clone(),
            }),
            _ => None,
        }
    }
}

#[async_trait::async_trait]
impl AuctionDirectoryReader for SqlxAuctionDirectoryReader {
    async fn list(
        &self,
        request: &ListAuctionsDirectoryRequest,
    ) -> Result<ListAuctionsDirectoryResult, AuctionDirectoryReadError> {
        let cursor = request.cursor.clone().unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize = usize::try_from(size).map_err(invalid_read_model)?;
        let limit = i64::try_from(size + 1).map_err(invalid_read_model)?;

        // Page roots first, then join their owned schedule points. One statement gives
        // every directory item one PostgreSQL statement snapshot without letting the
        // one-to-many schedule join change cursor limits.
        let mut builder = QueryBuilder::<Postgres>::new(
            "WITH selected AS (SELECT a.auction_id, a.listing_source_id, a.source_auction_id, a.name_text, a.name_language, a.description_text, a.description_language, a.catalogue_url, a.format, a.reported_status, a.reported_lot_count, a.version, a.created, a.updated, s.listing_source_slug_id, s.name AS listing_source_name FROM auctions a JOIN listing_sources s ON s.listing_source_id = a.listing_source_id",
        );
        if request.schedule.is_some() {
            builder.push(" JOIN auction_schedule_points schedule_filter ON schedule_filter.auction_id = a.auction_id");
        }
        builder.push(" WHERE TRUE");
        push_filters(&mut builder, request)?;
        if let Some(search_after) = cursor.search_after {
            builder
                .push(" AND (a.created, a.auction_id) < (")
                .push_bind(search_after.created)
                .push(", ")
                .push_bind(search_after.auction_id.as_uuid())
                .push(")");
        }
        builder
            .push(" ORDER BY a.created DESC, a.auction_id DESC LIMIT ")
            .push_bind(limit)
            .push(") SELECT selected.auction_id, selected.listing_source_id, selected.source_auction_id, selected.name_text, selected.name_language, selected.description_text, selected.description_language, selected.catalogue_url, selected.format, selected.reported_status, selected.reported_lot_count, selected.version, selected.created, selected.updated, selected.listing_source_slug_id, selected.listing_source_name, point.role, point.precision, point.instant_at, point.date_on, point.source_timezone FROM selected LEFT JOIN auction_schedule_points point ON point.auction_id = selected.auction_id ORDER BY selected.created DESC, selected.auction_id DESC, point.role");

        let mut connection = self.pool.acquire().await.map_err(query_error)?;
        let joined = builder
            .build_query_as::<JoinedAuctionDirectoryRow>()
            .fetch_all(&mut *connection)
            .await
            .map_err(query_error)?;
        let mut rows =
            HashMap::<uuid::Uuid, (AuctionDirectoryRow, Vec<AuctionSchedulePointRow>)>::new();
        let mut ordered_auction_ids = Vec::new();
        for row in joined {
            let auction_id = row.auction_id;
            let (_, schedule) = rows.entry(auction_id).or_insert_with(|| {
                ordered_auction_ids.push(auction_id);
                (row.auction(), Vec::new())
            });
            if let Some(point) = row.schedule_point() {
                schedule.push(point);
            }
        }
        let has_more = ordered_auction_ids.len() > size_usize;
        if has_more {
            ordered_auction_ids.truncate(size_usize);
        }
        if ordered_auction_ids.is_empty() {
            return Ok(CursoredResult {
                items: Vec::new(),
                cursor: Cursor {
                    size,
                    search_after: None,
                },
                total: None,
            });
        }

        let items = ordered_auction_ids
            .into_iter()
            .map(|auction_id| {
                let (row, schedule_rows) = rows.remove(&auction_id).ok_or_else(|| {
                    box_error(std::io::Error::other(
                        "selected Auction vanished during directory mapping",
                    ))
                })?;
                map_directory_item(row, schedule_rows)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| AuctionDirectoryReadError::InvalidReadModel { source })?;
        let search_after = has_more.then(|| {
            let item = &items[items.len() - 1];
            AuctionDirectoryCursor {
                created: item.created,
                auction_id: item.auction_id,
                scope: request.scope(),
            }
        });

        Ok(CursoredResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn push_filters(
    builder: &mut QueryBuilder<Postgres>,
    request: &ListAuctionsDirectoryRequest,
) -> Result<(), AuctionDirectoryReadError> {
    if let Some(listing_source_id) = request.listing_source_id {
        builder
            .push(" AND a.listing_source_id = ")
            .push_bind(listing_source_id.as_uuid());
    }
    if let Some(format) = request.format {
        builder.push(" AND a.format = ").push_bind(format.as_str());
    }
    if let Some(reported_status) = request.reported_status {
        builder
            .push(" AND a.reported_status = ")
            .push_bind(reported_status.as_str());
    }
    if let Some(schedule) = request.schedule.as_ref() {
        let (Some(min), Some(max)) = (schedule.range.min, schedule.range.max) else {
            return Err(AuctionDirectoryReadError::InvalidReadModel {
                source: box_error(std::io::Error::other(
                    "Auction directory schedule filter has incomplete exact-instant range",
                )),
            });
        };
        builder
            .push(" AND schedule_filter.role = ")
            .push_bind(schedule_role_code(schedule.role))
            .push(" AND schedule_filter.precision = 'INSTANT' AND schedule_filter.instant_at >= ")
            .push_bind(min)
            .push(" AND schedule_filter.instant_at < ")
            .push_bind(max);
    }
    Ok(())
}

fn schedule_role_code(role: AuctionSchedulePoint) -> &'static str {
    match role {
        AuctionSchedulePoint::BiddingOpens => "BIDDING_OPENS",
        AuctionSchedulePoint::LiveStarts => "LIVE_STARTS",
        AuctionSchedulePoint::LotsBeginClosing => "LOTS_BEGIN_CLOSING",
        AuctionSchedulePoint::ScheduledEnd => "SCHEDULED_END",
    }
}

fn map_directory_item(
    row: AuctionDirectoryRow,
    schedule_rows: Vec<AuctionSchedulePointRow>,
) -> Result<PublicAuctionDirectoryItem, BoxError> {
    let source_id = ListingSourceId::try_from(row.listing_source_id).map_err(box_error)?;
    let stored = map_stored_auction(
        AuctionRow {
            auction_id: row.auction_id,
            listing_source_id: row.listing_source_id,
            source_auction_id: row.source_auction_id,
            name_text: row.name_text,
            name_language: row.name_language,
            description_text: row.description_text,
            description_language: row.description_language,
            catalogue_url: row.catalogue_url,
            format: row.format,
            reported_status: row.reported_status,
            reported_lot_count: row.reported_lot_count,
            version: row.version,
            created: row.created,
            updated: row.updated,
        },
        schedule_rows,
    )
    .map_err(map_error)?;
    if stored.auction.key().listing_source_id() != source_id {
        return Err(box_error(std::io::Error::other(
            "persisted Auction source does not match its joined ListingSource",
        )));
    }
    let source = PublicAuctionDirectorySourceSummary {
        listing_source_id: source_id,
        slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id).map_err(box_error)?,
        name: ListingSourceName::try_from(row.listing_source_name).map_err(box_error)?,
    };
    let auction = stored.auction;
    Ok(PublicAuctionDirectoryItem {
        auction_id: auction.id(),
        source,
        name: auction.name().cloned(),
        format: auction.format(),
        schedule: auction.schedule().clone(),
        reported_status: auction.reported_status(),
        created: stored.created,
    })
}

fn query_error(error: sqlx::Error) -> AuctionDirectoryReadError {
    AuctionDirectoryReadError::QueryFailed {
        source: box_error(error),
    }
}

fn invalid_read_model(
    error: impl std::error::Error + Send + Sync + 'static,
) -> AuctionDirectoryReadError {
    AuctionDirectoryReadError::InvalidReadModel {
        source: box_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_exact_persisted_schedule_role_codes() {
        assert_eq!(
            "BIDDING_OPENS",
            schedule_role_code(AuctionSchedulePoint::BiddingOpens)
        );
        assert_eq!(
            "LIVE_STARTS",
            schedule_role_code(AuctionSchedulePoint::LiveStarts)
        );
        assert_eq!(
            "LOTS_BEGIN_CLOSING",
            schedule_role_code(AuctionSchedulePoint::LotsBeginClosing)
        );
        assert_eq!(
            "SCHEDULED_END",
            schedule_role_code(AuctionSchedulePoint::ScheduledEnd)
        );
    }
}
