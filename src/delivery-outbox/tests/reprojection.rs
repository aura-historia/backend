//! `reproject-all`: keyset paging, projection fences and the target index generation.

mod support;

use aura_historia_cdc_routing::product_listing_projection_job;
use aura_historia_jobs::wire;
use delivery_outbox::{ReprojectionReport, reproject_all};
use domain_primitives::event_id::EventId;
use platform_worker_queue_contract::job_queue::{ProjectionGeneration, ProjectionTarget};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_postgres::SqlxProductListingProjectionTriggerReader;
use sqlx::PgPool;
use std::num::NonZeroU32;
use support::{DATABASE, detach_foreign_keys};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

struct Listing {
    id: ProductListingId,
    current_event: EventId,
}

/// Listings in ID order; every third one is withdrawn.
async fn seed(pool: &PgPool, count: usize) -> Vec<Listing> {
    detach_foreign_keys(pool, &["product_listings"]).await;
    let mut listings = Vec::new();
    for index in 0..count {
        let listing = Listing {
            id: ProductListingId::new(),
            current_event: EventId::new(),
        };
        sqlx::query(
            "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, \
             current_event_id, content_source_event_id, embedding_source_event_id, \
             listing_source_id, source_listing_id, lifecycle, url) \
             VALUES ($1, $2, $3, $3, $3, $4, $2, $5, 'https://example.test/listing')",
        )
        .bind(listing.id.into_uuid())
        .bind(format!("listing-{index}-{index:06x}"))
        .bind(listing.current_event.into_uuid())
        .bind(uuid::Uuid::now_v7())
        .bind(if index % 3 == 2 {
            "WITHDRAWN"
        } else {
            "ACTIVE"
        })
        .execute(pool)
        .await
        .unwrap();
        listings.push(listing);
    }
    listings.sort_by_key(|listing| listing.id);
    listings
}

fn body(listing: &Listing) -> String {
    wire::encode(&product_listing_projection_job(
        listing.current_event,
        listing.id,
    ))
    .unwrap()
}

async fn queued(pool: &PgPool, scope: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT payload FROM job_queue_jobs WHERE scope = $1 ORDER BY sequence")
        .bind(scope)
        .fetch_all(pool)
        .await
        .unwrap()
}

fn page(size: u32) -> NonZeroU32 {
    NonZeroU32::new(size).unwrap()
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_enqueue_one_current_event_job_per_listing_in_keyset_pages() {
    let pool = get_postgres_client().await;
    let listings = seed(&pool, 7).await;
    let reader = SqlxProductListingProjectionTriggerReader::new(pool.clone());

    let report = reproject_all(&reader, &pool, &ProjectionTarget::Live, page(3), None)
        .await
        .unwrap();

    assert_eq!(
        ReprojectionReport {
            pages: 3,
            enqueued: 7,
            deduplicated: 0,
            last: listings.last().map(|listing| listing.id),
        },
        report
    );
    assert_eq!(
        listings.iter().map(body).collect::<Vec<_>>(),
        queued(&pool, "product-listing-opensearch").await,
        "every listing, withdrawn ones included, with its current event, in ID order"
    );
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_stop_after_an_exactly_full_last_page_and_resume_after_a_listing() {
    let pool = get_postgres_client().await;
    let listings = seed(&pool, 6).await;
    let reader = SqlxProductListingProjectionTriggerReader::new(pool.clone());

    let resumed = reproject_all(
        &reader,
        &pool,
        &ProjectionTarget::Live,
        page(2),
        Some(listings[1].id),
    )
    .await
    .unwrap();

    assert_eq!((2, 4), (resumed.pages, resumed.enqueued));
    assert_eq!(
        listings[2..].iter().map(body).collect::<Vec<_>>(),
        queued(&pool, "product-listing-opensearch").await
    );
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_add_only_changed_listings_when_rerun_before_the_jobs_ran() {
    let pool = get_postgres_client().await;
    let mut listings = seed(&pool, 4).await;
    let reader = SqlxProductListingProjectionTriggerReader::new(pool.clone());
    reproject_all(&reader, &pool, &ProjectionTarget::Live, page(10), None)
        .await
        .unwrap();
    listings[1].current_event = EventId::new();
    sqlx::query("UPDATE product_listings SET current_event_id = $1 WHERE product_listing_id = $2")
        .bind(listings[1].current_event.into_uuid())
        .bind(listings[1].id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let rerun = reproject_all(&reader, &pool, &ProjectionTarget::Live, page(10), None)
        .await
        .unwrap();

    assert_eq!((1, 3), (rerun.enqueued, rerun.deduplicated));
    assert_eq!(
        Some(&body(&listings[1])),
        queued(&pool, "product-listing-opensearch").await.last()
    );
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_send_a_generation_reprojection_only_to_that_generations_scope() {
    let pool = get_postgres_client().await;
    let listings = seed(&pool, 3).await;
    let reader = SqlxProductListingProjectionTriggerReader::new(pool.clone());
    let target = ProjectionTarget::Generation(ProjectionGeneration::new("rehearsal1").unwrap());

    reproject_all(&reader, &pool, &target, page(10), None)
        .await
        .unwrap();

    assert_eq!(
        listings.iter().map(body).collect::<Vec<_>>(),
        queued(&pool, "product-listing-opensearch-rehearsal1").await
    );
    assert!(queued(&pool, "product-listing-opensearch").await.is_empty());
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_finish_without_jobs_when_there_are_no_listings() {
    let pool = get_postgres_client().await;
    let reader = SqlxProductListingProjectionTriggerReader::new(pool.clone());

    let report = reproject_all(&reader, &pool, &ProjectionTarget::Live, page(10), None)
        .await
        .unwrap();

    assert_eq!(ReprojectionReport::default(), report);
}
