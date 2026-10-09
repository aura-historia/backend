use crate::mapping::{AuctionRow, map_error, map_stored_auction};
use application::{
    error::{BoxError, box_error},
    pagination::{Cursor, CursoredResult},
};
use auction_service::ports::admin_auction_search_reader::{
    AdminAuctionSearchCursor, AdminAuctionSearchItem, AdminAuctionSearchReadError,
    AdminAuctionSearchReadResult, AdminAuctionSearchReader, AdminAuctionSearchRequest,
    AdminAuctionSort,
};
use domain_primitives::sort::SortOrder;
use sqlx::{PgPool, Postgres, QueryBuilder};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone)]
pub struct SqlxAdminAuctionSearchReader {
    pool: PgPool,
}

impl SqlxAdminAuctionSearchReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct SearchRow {
    auction_id: Uuid,
    listing_source_id: Uuid,
    source_auction_id: String,
    name_text: Option<String>,
    name_language: Option<String>,
    catalogue_url: Option<String>,
    format: Option<String>,
    bidding_opens_at: Option<OffsetDateTime>,
    live_starts_at: Option<OffsetDateTime>,
    lots_begin_closing_at: Option<OffsetDateTime>,
    scheduled_end_at: Option<OffsetDateTime>,
    reported_status: Option<String>,
    reported_lot_count: Option<i64>,
    version: i64,
    created: OffsetDateTime,
    updated: OffsetDateTime,
    sort_name: Option<String>,
}

impl SearchRow {
    fn into_auction_row(self) -> AuctionRow {
        AuctionRow {
            auction_id: self.auction_id,
            listing_source_id: self.listing_source_id,
            source_auction_id: self.source_auction_id,
            name_text: self.name_text,
            name_language: self.name_language,
            catalogue_url: self.catalogue_url,
            format: self.format,
            bidding_opens_at: self.bidding_opens_at,
            live_starts_at: self.live_starts_at,
            lots_begin_closing_at: self.lots_begin_closing_at,
            scheduled_end_at: self.scheduled_end_at,
            reported_status: self.reported_status,
            reported_lot_count: self.reported_lot_count,
            version: self.version,
            created: self.created,
            updated: self.updated,
        }
    }
}

#[async_trait::async_trait]
impl AdminAuctionSearchReader for SqlxAdminAuctionSearchReader {
    async fn search(
        &self,
        request: &AdminAuctionSearchRequest,
    ) -> Result<AdminAuctionSearchReadResult, AdminAuctionSearchReadError> {
        let size = request.cursor.size;
        if !(1..=100).contains(&size) {
            return Err(invalid_read_model(std::io::Error::other(
                "invalid Auction search page size",
            )));
        }
        let limit = i64::try_from(size + 1).map_err(invalid_read_model)?;
        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT a.auction_id, a.listing_source_id, a.source_auction_id, a.name_text, a.name_language, a.catalogue_url, a.format, a.bidding_opens_at, a.live_starts_at, a.lots_begin_closing_at, a.scheduled_end_at, a.reported_status, a.reported_lot_count, a.version, a.created, a.updated, lower(a.name_text) COLLATE \"C\" AS sort_name FROM auctions a WHERE TRUE",
        );
        push_filters(&mut builder, request);
        push_page(&mut builder, request);
        builder.push(" LIMIT ").push_bind(limit);
        let rows = builder
            .build_query_as::<SearchRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(|source| AdminAuctionSearchReadError::QueryFailed {
                source: box_error(source),
            })?;
        let has_more = rows.len() > size as usize;
        let mut items = Vec::with_capacity(rows.len().min(size as usize));
        let mut last_name = None;
        for row in rows.into_iter().take(size as usize) {
            last_name = row.sort_name.clone();
            items.push(
                map_item(row)
                    .map_err(|source| AdminAuctionSearchReadError::InvalidReadModel { source })?,
            );
        }
        let search_after = if has_more {
            let last = items.last().expect("a page with more rows has a last item");
            Some(AdminAuctionSearchCursor {
                auction_id: last.auction_id,
                sort_name: (request.scope.sort == AdminAuctionSort::Name)
                    .then_some(last_name)
                    .flatten(),
                created: last.created,
                updated: last.updated,
                scope: request.scope.clone(),
            })
        } else {
            None
        };
        Ok(CursoredResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn push_filters(builder: &mut QueryBuilder<Postgres>, request: &AdminAuctionSearchRequest) {
    let scope = &request.scope;
    if let Some(query) = &scope.query {
        // strpos treats %, _ and backslashes as literal characters, unlike ILIKE patterns.
        builder
            .push(" AND (strpos(lower(a.name_text), lower(")
            .push_bind(query)
            .push(")) > 0 OR strpos(lower(a.source_auction_id), lower(")
            .push_bind(query)
            .push(")) > 0)");
    }
    if let Some(id) = scope.listing_source_id {
        builder
            .push(" AND a.listing_source_id = ")
            .push_bind(id.as_uuid());
    }
    if let Some(id) = &scope.source_auction_id {
        builder
            .push(" AND a.source_auction_id = ")
            .push_bind(id.as_ref());
    }
    if let Some(format) = scope.format {
        builder.push(" AND a.format = ").push_bind(format.as_str());
    }
    if let Some(status) = scope.reported_status {
        builder
            .push(" AND a.reported_status = ")
            .push_bind(status.as_str());
    }
}

fn push_page(builder: &mut QueryBuilder<Postgres>, request: &AdminAuctionSearchRequest) {
    let order = request.scope.order;
    let cmp = if order == SortOrder::Asc {
        " > "
    } else {
        " < "
    };
    let direction = if order == SortOrder::Asc {
        " ASC"
    } else {
        " DESC"
    };
    let sort = request.scope.sort;
    let after = request.cursor.search_after.as_ref();
    if sort == AdminAuctionSort::Name {
        // NULL names always trail named Auctions, for both directions.
        if let Some(after) = after {
            if let Some(name) = &after.sort_name {
                // SELECT aliases are not visible in WHERE; repeat the exact sort expression.
                builder
                    .push(" AND ((lower(a.name_text) COLLATE \"C\") ")
                    .push(cmp)
                    .push_bind(name);
                builder
                    .push(" OR ((lower(a.name_text) COLLATE \"C\") = ")
                    .push_bind(name);
                builder
                    .push(" AND a.auction_id ")
                    .push(cmp)
                    .push_bind(after.auction_id.as_uuid());
                builder.push(") OR a.name_text IS NULL)");
            } else {
                builder
                    .push(" AND a.name_text IS NULL AND a.auction_id ")
                    .push(cmp)
                    .push_bind(after.auction_id.as_uuid());
            }
        }
        builder
            .push(" ORDER BY lower(a.name_text) COLLATE \"C\"")
            .push(direction)
            .push(" NULLS LAST, a.auction_id")
            .push(direction);
    } else {
        let column = if sort == AdminAuctionSort::Created {
            "a.created"
        } else {
            "a.updated"
        };
        if let Some(after) = after {
            builder
                .push(" AND (")
                .push(column)
                .push(", a.auction_id) ")
                .push(cmp)
                .push(" (");
            builder.push_bind(if sort == AdminAuctionSort::Created {
                after.created
            } else {
                after.updated
            });
            builder
                .push(", ")
                .push_bind(after.auction_id.as_uuid())
                .push(")");
        }
        builder
            .push(" ORDER BY ")
            .push(column)
            .push(direction)
            .push(", a.auction_id")
            .push(direction);
    }
}

fn map_item(row: SearchRow) -> Result<AdminAuctionSearchItem, BoxError> {
    let stored = map_stored_auction(row.into_auction_row()).map_err(map_error)?;
    let auction = stored.auction;
    Ok(AdminAuctionSearchItem {
        auction_id: auction.id(),
        key: auction.key().clone(),
        name: auction.name().cloned(),
        catalogue_url: auction.catalogue_url().cloned(),
        format: auction.format(),
        schedule: auction.schedule().clone(),
        reported_status: auction.reported_status(),
        reported_lot_count: auction.reported_lot_count(),
        version: stored.version,
        created: stored.created,
        updated: stored.updated,
    })
}

fn invalid_read_model(
    source: impl std::error::Error + Send + Sync + 'static,
) -> AdminAuctionSearchReadError {
    AdminAuctionSearchReadError::InvalidReadModel {
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auction_core::{AuctionFormat, AuctionId, AuctionReportedStatus, SourceAuctionId};
    use auction_service::ports::admin_auction_search_reader::AdminAuctionSearchScope;
    use listing_source_core::ListingSourceId;
    use test_api::{
        IntegrationTestService, Postgres as TestPostgres, aura_integration_test,
        get_postgres_client,
    };
    use time::macros::datetime;

    const BUSINESS_SCHEMA: TestPostgres = TestPostgres::new("migrations");

    async fn seed_source(pool: &PgPool) -> ListingSourceId {
        let id = ListingSourceId::new();
        let party_id = Uuid::now_v7();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(party_id)
            .bind(format!("party-{party_id}"))
            .bind("Auction admin search test")
            .execute(pool)
            .await
            .expect("seed party");
        sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
            .bind(id.as_uuid()).bind(format!("source-{}", id.as_uuid())).bind("Auction admin search test").bind(party_id)
            .execute(pool).await.expect("seed listing source");
        id
    }

    async fn seed_auction(
        pool: &PgPool,
        source: ListingSourceId,
        source_key: &str,
        name: Option<&str>,
        format: Option<&str>,
        reported_status: Option<&str>,
        updated: OffsetDateTime,
    ) -> AuctionId {
        let id = AuctionId::new();
        sqlx::query("INSERT INTO auctions (auction_id, listing_source_id, source_auction_id, name_text, name_language, format, reported_status, created, updated) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
            .bind(id.as_uuid()).bind(source.as_uuid()).bind(source_key).bind(name)
            .bind(name.map(|_| "en")).bind(format).bind(reported_status).bind(updated).bind(updated)
            .execute(pool).await.expect("seed auction");
        id
    }

    fn request(
        source: ListingSourceId,
        sort: AdminAuctionSort,
        order: SortOrder,
    ) -> AdminAuctionSearchRequest {
        AdminAuctionSearchRequest {
            scope: AdminAuctionSearchScope {
                query: None,
                listing_source_id: Some(source),
                source_auction_id: None,
                format: None,
                reported_status: None,
                sort,
                order,
            },
            cursor: Cursor {
                size: 1,
                search_after: None,
            },
        }
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn pages_all_auctions_by_updated_created_and_name_in_both_orders() {
        let pool = get_postgres_client().await;
        let source = seed_source(&pool).await;
        let now = datetime!(2026-09-01 00:00 UTC);
        let a = seed_auction(&pool, source, "one", Some("Same"), Some("LIVE"), None, now).await;
        let b = seed_auction(&pool, source, "two", Some("same"), Some("LIVE"), None, now).await;
        let c = seed_auction(
            &pool,
            source,
            "three",
            None,
            None,
            None,
            datetime!(2026-08-31 00:00 UTC),
        )
        .await;
        let d = seed_auction(
            &pool,
            source,
            "four",
            None,
            None,
            None,
            datetime!(2026-08-31 00:00 UTC),
        )
        .await;
        let e = seed_auction(
            &pool,
            source,
            "five",
            None,
            None,
            None,
            datetime!(2026-08-31 00:00 UTC),
        )
        .await;
        let mut unnamed = [c, d, e];
        unnamed.sort();
        let reader = SqlxAdminAuctionSearchReader::new(pool);
        for sort in [
            AdminAuctionSort::Name,
            AdminAuctionSort::Created,
            AdminAuctionSort::Updated,
        ] {
            for order in [SortOrder::Asc, SortOrder::Desc] {
                let expected = match (sort, order) {
                    (AdminAuctionSort::Name, SortOrder::Asc) => {
                        vec![a.min(b), a.max(b), unnamed[0], unnamed[1], unnamed[2]]
                    }
                    (_, SortOrder::Asc) => {
                        vec![unnamed[0], unnamed[1], unnamed[2], a.min(b), a.max(b)]
                    }
                    (_, SortOrder::Desc) => {
                        vec![a.max(b), a.min(b), unnamed[2], unnamed[1], unnamed[0]]
                    }
                };
                let mut request = request(source, sort, order);
                let mut seen = Vec::new();
                for _ in 0..expected.len() {
                    let page = reader.search(&request).await.expect("search auctions");
                    assert_eq!(1, page.items.len());
                    seen.extend(page.items.iter().map(|item| item.auction_id));
                    request.cursor.search_after = page.cursor.search_after;
                    if request.cursor.search_after.is_none() {
                        break;
                    }
                }
                assert_eq!(expected, seen, "sort={sort:?}, order={order:?}");
                assert!(
                    request.cursor.search_after.is_none(),
                    "cursor must terminate"
                );
            }
        }
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn filters_format_independently_including_unspecified_format() {
        let pool = get_postgres_client().await;
        let source = seed_source(&pool).await;
        let now = datetime!(2026-09-01 00:00 UTC);
        let live = seed_auction(
            &pool,
            source,
            "live",
            Some("Matching Auction"),
            Some("LIVE"),
            Some("ENDED"),
            now,
        )
        .await;
        let timed = seed_auction(
            &pool,
            source,
            "timed",
            Some("Matching Auction"),
            Some("TIMED"),
            Some("ENDED"),
            now,
        )
        .await;
        seed_auction(
            &pool,
            source,
            "unspecified",
            Some("Matching Auction"),
            None,
            Some("ENDED"),
            now,
        )
        .await;
        let reader = SqlxAdminAuctionSearchReader::new(pool);
        let mut request = request(source, AdminAuctionSort::Updated, SortOrder::Desc);
        request.scope.query = Some("matching".into());
        request.cursor.size = 100;
        for (format, expected) in [(AuctionFormat::Live, live), (AuctionFormat::Timed, timed)] {
            request.scope.format = Some(format);
            let page = reader.search(&request).await.expect("search by format");
            assert_eq!(
                vec![expected],
                page.items
                    .into_iter()
                    .map(|item| item.auction_id)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn filters_reported_status_independently_including_unspecified_status() {
        let pool = get_postgres_client().await;
        let source = seed_source(&pool).await;
        let now = datetime!(2026-09-01 00:00 UTC);
        let ended = seed_auction(
            &pool,
            source,
            "ended",
            Some("Matching Auction"),
            Some("LIVE"),
            Some("ENDED"),
            now,
        )
        .await;
        let scheduled = seed_auction(
            &pool,
            source,
            "scheduled",
            Some("Matching Auction"),
            Some("LIVE"),
            Some("SCHEDULED"),
            now,
        )
        .await;
        seed_auction(
            &pool,
            source,
            "unspecified",
            Some("Matching Auction"),
            Some("LIVE"),
            None,
            now,
        )
        .await;
        let reader = SqlxAdminAuctionSearchReader::new(pool);
        let mut request = request(source, AdminAuctionSort::Updated, SortOrder::Desc);
        request.scope.query = Some("matching".into());
        request.cursor.size = 100;
        for (status, expected) in [
            (AuctionReportedStatus::Ended, ended),
            (AuctionReportedStatus::Scheduled, scheduled),
        ] {
            request.scope.reported_status = Some(status);
            let page = reader
                .search(&request)
                .await
                .expect("search by reported status");
            assert_eq!(
                vec![expected],
                page.items
                    .into_iter()
                    .map(|item| item.auction_id)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn searches_literal_case_insensitive_text_and_exact_source_keys_without_listings() {
        let pool = get_postgres_client().await;
        let source = seed_source(&pool).await;
        let other = seed_source(&pool).await;
        let now = datetime!(2026-09-01 00:00 UTC);
        let hit = seed_auction(
            &pool,
            source,
            "Sale%42",
            Some("Hidden _ Auction"),
            Some("LIVE"),
            None,
            now,
        )
        .await;
        seed_auction(
            &pool,
            source,
            "other",
            Some("Hidden Auction"),
            Some("TIMED"),
            None,
            now,
        )
        .await;
        seed_auction(
            &pool,
            other,
            "Sale%42",
            Some("Hidden _ Auction"),
            Some("LIVE"),
            None,
            now,
        )
        .await;
        let reader = SqlxAdminAuctionSearchReader::new(pool);
        let mut request = request(source, AdminAuctionSort::Updated, SortOrder::Desc);
        request.scope.query = Some("%42".into());
        request.scope.source_auction_id = Some(SourceAuctionId::try_from("Sale%42").expect("key"));
        request.scope.format = Some(AuctionFormat::Live);
        assert_eq!(
            vec![hit],
            reader
                .search(&request)
                .await
                .expect("search")
                .items
                .into_iter()
                .map(|item| item.auction_id)
                .collect::<Vec<_>>()
        );
        request.scope.query = Some("_ aUcTiOn".into());
        assert_eq!(
            vec![hit],
            reader
                .search(&request)
                .await
                .expect("search")
                .items
                .into_iter()
                .map(|item| item.auction_id)
                .collect::<Vec<_>>()
        );
    }
}
