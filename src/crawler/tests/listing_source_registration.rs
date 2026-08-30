use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
    RegisteredListingSource,
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

fn listing_source(
    listing_source_id: ListingSourceId,
    crawl_enabled: bool,
) -> RegisteredListingSource {
    RegisteredListingSource {
        listing_source_id,
        listing_source_name: ListingSourceName::try_from("Test source").unwrap(),
        listing_source_slug: ListingSourceSlugId::raw("test-source").unwrap(),
        crawl_enabled,
    }
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_disable_absent_sources_without_deleting_local_domain_configuration() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let kept = ListingSourceId::new();
    let removed = ListingSourceId::new();
    repository
        .apply_snapshot(&[listing_source(kept, true), listing_source(removed, true)])
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO listing_source_domains (listing_source_id, listing_source_domain) VALUES ($1, $2)",
    )
    .bind(uuid::Uuid::from(removed))
    .bind("removed.example.com")
    .execute(&pool)
    .await
    .unwrap();

    let result = repository
        .apply_snapshot(&[listing_source(kept, true)])
        .await
        .unwrap();
    assert_eq!(result.disabled, 1);
    let enabled: bool = sqlx::query_scalar(
        "SELECT crawl_enabled FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(uuid::Uuid::from(removed))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!enabled);
    let domain_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listing_source_domains WHERE listing_source_id = $1",
    )
    .bind(uuid::Uuid::from(removed))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(domain_count, 1);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_default_ad_hoc_listing_sources_to_crawl_disabled() {
    let pool = get_postgres_client().await;
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug) \
         VALUES ($1, 'Ad hoc', 'ad-hoc')",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .execute(&pool)
    .await
    .unwrap();
    let enabled: bool = sqlx::query_scalar(
        "SELECT crawl_enabled FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!enabled);
}
