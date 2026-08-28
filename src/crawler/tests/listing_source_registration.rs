use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
    RegisteredListingSource,
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

fn listing_source(listing_source_id: ListingSourceId) -> RegisteredListingSource {
    RegisteredListingSource {
        listing_source_id,
        listing_source_name: ListingSourceName::try_from("Test source")
            .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
        listing_source_slug: ListingSourceSlugId::raw("test-source")
            .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
        crawl_enabled: true,
    }
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_disable_local_configuration_when_source_leaves_crawl_scope() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let kept_id = ListingSourceId::new();
    let removed_id = ListingSourceId::new();

    repository
        .upsert_listing_source(&listing_source(kept_id))
        .await
        .unwrap();
    repository
        .upsert_listing_source(&listing_source(removed_id))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO listing_source_domains (listing_source_id, listing_source_domain) VALUES ($1, $2)",
    )
    .bind(uuid::Uuid::from(removed_id))
    .bind("removed.example.com")
    .execute(&pool)
    .await
    .unwrap();

    let disabled = repository
        .disable_listing_sources_not_in(&[kept_id])
        .await
        .unwrap();

    assert_eq!(disabled, 1);
    let crawl_enabled: bool = sqlx::query_scalar(
        "SELECT crawl_enabled FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(uuid::Uuid::from(removed_id))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!crawl_enabled);
    let remaining_domains: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM listing_source_domains WHERE listing_source_id = $1")
            .bind(uuid::Uuid::from(removed_id))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining_domains.0, 1);
}
