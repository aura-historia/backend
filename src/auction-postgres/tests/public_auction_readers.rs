use application::pagination::Cursor;
use auction_core::{AuctionId, AuctionSchedulePoint};
use auction_postgres::{SqlxAuctionDirectoryReader, SqlxPublicAuctionDetailsReader};
use auction_service::ports::{
    AuctionDirectoryReader, AuctionInstantScheduleFilter, ListAuctionsDirectoryRequest,
    PublicAuctionDetailsReader,
};
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::sort::SortOrder;
use listing_source_core::ListingSourceId;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{OffsetDateTime, macros::datetime};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_filter_and_page_directory_by_schedule_role_with_half_open_bounds() {
    let pool = get_postgres_client().await;
    let source_id = seed_listing_source(&pool, "directory-schedule-source").await;
    let from = datetime!(2026-10-18 16:00 UTC);
    let to = datetime!(2026-10-18 17:00 UTC);
    let included = seed_auction(&pool, source_id, "directory-included", 0).await;
    let tie_a = seed_auction(&pool, source_id, "directory-tie-a", 0).await;
    let tie_b = seed_auction(&pool, source_id, "directory-tie-b", 0).await;
    let excluded_lower = seed_auction(&pool, source_id, "directory-lower", 0).await;
    let excluded_upper = seed_auction(&pool, source_id, "directory-upper", 0).await;
    let _null = seed_auction(&pool, source_id, "directory-null", 0).await;
    let wrong_role = seed_auction(&pool, source_id, "directory-wrong-role", 0).await;
    seed_schedule(&pool, included, "LIVE_STARTS", from).await;
    for id in [tie_a, tie_b] {
        seed_schedule(&pool, id, "LIVE_STARTS", datetime!(2026-10-18 16:30 UTC)).await;
    }
    seed_schedule(
        &pool,
        excluded_lower,
        "LIVE_STARTS",
        datetime!(2026-10-18 15:59:59 UTC),
    )
    .await;
    seed_schedule(&pool, excluded_upper, "LIVE_STARTS", to).await;
    seed_schedule(&pool, wrong_role, "SCHEDULED_END", from).await;

    let reader = SqlxAuctionDirectoryReader::new(pool);
    let page = reader
        .list(&ListAuctionsDirectoryRequest {
            schedule: Some(AuctionInstantScheduleFilter {
                role: AuctionSchedulePoint::LiveStarts,
                range: RangeQuery {
                    min: Some(from),
                    max: Some(to),
                },
            }),
            cursor: Some(Cursor {
                size: 10,
                search_after: None,
            }),
            ..Default::default()
        })
        .await
        .unwrap_or_else(|error| panic!("failed to list auction directory: {error:?}"));

    assert_eq!(3, page.items.len());
    for id in [included, tie_a, tie_b] {
        assert!(page.items.iter().any(|item| item.auction_id == id));
    }

    let mut ties = [tie_a, tie_b];
    ties.sort_by(|left, right| left.as_uuid().cmp(right.as_uuid()));
    for (order, expected) in [
        (SortOrder::Asc, [included, ties[0], ties[1]]),
        (SortOrder::Desc, [ties[1], ties[0], included]),
    ] {
        assert_single_item_directory_pages(
            &reader,
            ListAuctionsDirectoryRequest {
                listing_source_id: Some(source_id),
                schedule: Some(AuctionInstantScheduleFilter {
                    role: AuctionSchedulePoint::LiveStarts,
                    range: RangeQuery {
                        min: Some(from),
                        max: Some(to),
                    },
                }),
                sort: Some(AuctionSchedulePoint::LiveStarts),
                order: Some(order),
                ..Default::default()
            },
            &expected,
        )
        .await;
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_page_scheduled_directory_in_both_orders_with_nulls_last_and_id_ties() {
    let pool = get_postgres_client().await;
    let source_id = seed_listing_source(&pool, "directory-scheduled-page-source").await;
    let early = seed_auction(&pool, source_id, "schedule-early", 0).await;
    let tie_a = seed_auction(&pool, source_id, "schedule-tie-a", 0).await;
    let tie_b = seed_auction(&pool, source_id, "schedule-tie-b", 0).await;
    let null_a = seed_auction(&pool, source_id, "schedule-null-a", 0).await;
    let null_b = seed_auction(&pool, source_id, "schedule-null-b", 0).await;
    seed_schedule(&pool, early, "LIVE_STARTS", datetime!(2026-10-18 15:00 UTC)).await;
    seed_schedule(&pool, tie_a, "LIVE_STARTS", datetime!(2026-10-18 16:00 UTC)).await;
    seed_schedule(&pool, tie_b, "LIVE_STARTS", datetime!(2026-10-18 16:00 UTC)).await;
    let reader = SqlxAuctionDirectoryReader::new(pool);
    let mut ties = [tie_a, tie_b];
    ties.sort_by(|left, right| left.as_uuid().cmp(right.as_uuid()));
    let mut nulls = [null_a, null_b];
    nulls.sort_by(|left, right| left.as_uuid().cmp(right.as_uuid()));
    for (order, expected) in [
        (
            SortOrder::Asc,
            vec![early, ties[0], ties[1], nulls[0], nulls[1]],
        ),
        (
            SortOrder::Desc,
            vec![ties[1], ties[0], early, nulls[1], nulls[0]],
        ),
    ] {
        assert_single_item_directory_pages(
            &reader,
            ListAuctionsDirectoryRequest {
                listing_source_id: Some(source_id),
                sort: Some(AuctionSchedulePoint::LiveStarts),
                order: Some(order),
                ..Default::default()
            },
            &expected,
        )
        .await;
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_page_created_directory_in_both_orders_with_timestamp_ties() {
    let pool = get_postgres_client().await;
    let source_id = seed_listing_source(&pool, "directory-created-page-source").await;
    let ids = [
        seed_auction(&pool, source_id, "created-one", 0).await,
        seed_auction(&pool, source_id, "created-two", 0).await,
        seed_auction(&pool, source_id, "created-three", 0).await,
        seed_auction(&pool, source_id, "created-four", 0).await,
    ];
    for (id, created) in [
        (ids[0], datetime!(2026-01-01 00:00 UTC)),
        (ids[1], datetime!(2026-01-02 00:00 UTC)),
        (ids[2], datetime!(2026-01-01 00:00 UTC)),
        (ids[3], datetime!(2026-01-02 00:00 UTC)),
    ] {
        sqlx::query("UPDATE auctions SET created = $1 WHERE auction_id = $2")
            .bind(created)
            .bind(id.as_uuid())
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to set creation instant: {error}"));
    }
    let reader = SqlxAuctionDirectoryReader::new(pool);
    let mut early = [ids[0], ids[2]];
    early.sort_by(|left, right| left.as_uuid().cmp(right.as_uuid()));
    let mut late = [ids[1], ids[3]];
    late.sort_by(|left, right| left.as_uuid().cmp(right.as_uuid()));
    for (order, expected) in [
        (SortOrder::Asc, [early[0], early[1], late[0], late[1]]),
        (SortOrder::Desc, [late[1], late[0], early[1], early[0]]),
    ] {
        assert_single_item_directory_pages(
            &reader,
            ListAuctionsDirectoryRequest {
                listing_source_id: Some(source_id),
                order: Some(order),
                ..Default::default()
            },
            &expected,
        )
        .await;
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_reported_lot_count_separately_from_visible_active_assigned_listing_count() {
    let pool = get_postgres_client().await;
    let source_id = seed_listing_source(&pool, "details-count-source").await;
    let auction_id = seed_auction(&pool, source_id, "details-count", 7).await;
    seed_assigned_listing(&pool, source_id, auction_id, "details-active", "ACTIVE").await;
    seed_assigned_listing(
        &pool,
        source_id,
        auction_id,
        "details-withdrawn",
        "WITHDRAWN",
    )
    .await;

    let details = SqlxPublicAuctionDetailsReader::new(pool)
        .find_by_id(auction_id)
        .await
        .unwrap_or_else(|error| panic!("failed to read public auction details: {error:?}"))
        .unwrap_or_else(|| panic!("missing seeded auction"));

    assert_eq!(
        Some(7),
        details.reported_lot_count.map(|count| count.value())
    );
    assert_eq!(1, details.visible_active_assigned_listing_count);
    assert_eq!(auction_id, details.auction_id);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_none_for_missing_public_auction_details() {
    let pool = get_postgres_client().await;

    let details = SqlxPublicAuctionDetailsReader::new(pool)
        .find_by_id(AuctionId::new())
        .await
        .unwrap_or_else(|error| panic!("failed to query missing public auction: {error:?}"));

    assert!(details.is_none());
}

async fn assert_single_item_directory_pages(
    reader: &SqlxAuctionDirectoryReader,
    mut request: ListAuctionsDirectoryRequest,
    expected: &[AuctionId],
) {
    request.cursor = Some(Cursor {
        size: 1,
        search_after: None,
    });
    for (index, expected_id) in expected.iter().enumerate() {
        let page = reader
            .list(&request)
            .await
            .unwrap_or_else(|error| panic!("failed to page auction directory: {error:?}"));
        assert_eq!(
            vec![*expected_id],
            page.items
                .iter()
                .map(|item| item.auction_id)
                .collect::<Vec<_>>()
        );
        assert_eq!(1, page.cursor.size);
        assert_eq!(
            index + 1 < expected.len(),
            page.cursor.search_after.is_some()
        );
        if let Some(after) = page.cursor.search_after.as_ref() {
            assert_eq!(*expected_id, after.auction_id);
            assert_eq!(request.scope(), after.scope);
            if request.sort == Some(AuctionSchedulePoint::LiveStarts) {
                assert_eq!(page.items[0].schedule.live_starts(), after.scheduled);
            }
        }
        request.cursor = Some(page.cursor);
    }
}

async fn seed_listing_source(pool: &sqlx::PgPool, slug: &str) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    let party_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing-source party: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(listing_source_id.into_uuid())
        .bind(slug)
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing source: {error}"));
    listing_source_id
}

async fn seed_auction(
    pool: &sqlx::PgPool,
    source_id: ListingSourceId,
    source_auction_id: &str,
    reported_lot_count: i64,
) -> AuctionId {
    let auction_id = AuctionId::new();
    sqlx::query("INSERT INTO auctions (auction_id, listing_source_id, source_auction_id, name_text, name_language, format, reported_status, reported_lot_count) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
        .bind(auction_id.as_uuid())
        .bind(source_id.into_uuid())
        .bind(source_auction_id)
        .bind("Public auction")
        .bind("en")
        .bind("TIMED")
        .bind("SCHEDULED")
        .bind(reported_lot_count)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed auction: {error}"));
    auction_id
}

async fn seed_schedule(
    pool: &sqlx::PgPool,
    auction_id: AuctionId,
    role: &str,
    instant: OffsetDateTime,
) {
    let query = match role {
        "BIDDING_OPENS" => "UPDATE auctions SET bidding_opens_at = $1 WHERE auction_id = $2",
        "LIVE_STARTS" => "UPDATE auctions SET live_starts_at = $1 WHERE auction_id = $2",
        "LOTS_BEGIN_CLOSING" => {
            "UPDATE auctions SET lots_begin_closing_at = $1 WHERE auction_id = $2"
        }
        "SCHEDULED_END" => "UPDATE auctions SET scheduled_end_at = $1 WHERE auction_id = $2",
        _ => panic!("invalid Auction schedule role"),
    };
    sqlx::query(query)
        .bind(instant)
        .bind(auction_id.as_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed schedule: {error}"));
}

async fn seed_assigned_listing(
    pool: &sqlx::PgPool,
    source_id: ListingSourceId,
    auction_id: AuctionId,
    source_listing_id: &str,
    lifecycle: &str,
) {
    let listing_id = uuid::Uuid::now_v7();
    let event_id = uuid::Uuid::now_v7();
    let mut transaction = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin listing fixture transaction: {error}"));
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, availability, lifecycle, url, product_images, auction_id) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, $7, $8, '[]', $9)",
    )
    .bind(listing_id)
    .bind(format!("{source_listing_id}-000001"))
    .bind(event_id)
    .bind(source_id.into_uuid())
    .bind(source_listing_id)
    .bind((lifecycle == "ACTIVE").then_some("AVAILABLE"))
    .bind(lifecycle)
    .bind(format!("https://example.com/{source_listing_id}"))
    .bind(auction_id.as_uuid())
    .execute(&mut *transaction)
    .await
    .unwrap_or_else(|error| panic!("failed to seed assigned listing: {error}"));
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, '{}', now())")
        .bind(event_id)
        .bind(listing_id)
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing event: {error}"));

    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit listing fixture transaction: {error}"));
}
