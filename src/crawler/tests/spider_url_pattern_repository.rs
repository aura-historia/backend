use crawler::spider::classification::url_pattern_repository::{
    ListingSourceUrlPatternRepository, ListingSourceUrlPatternRepositoryImpl,
};
use listing_source_core::ListingSourceId;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

async fn insert_source(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) {
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled) \
         VALUES ($1, 'Test source', 'test-source', TRUE)",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_domain(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
    domain: &str,
) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO listing_source_domains (listing_source_id, listing_source_domain) \
         VALUES ($1, $2) RETURNING domain_id",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(domain)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_store_patterns_independently_for_domains_of_one_listing_source() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_a = insert_domain(&pool, listing_source_id, "a.example.com").await;
    let domain_b = insert_domain(&pool, listing_source_id, "b.example.com").await;

    repository
        .save_pattern(&listing_source_id, &domain_a, Some(r"/products/"))
        .await
        .unwrap();
    repository
        .save_pattern(&listing_source_id, &domain_b, Some(r"/objects/"))
        .await
        .unwrap();

    let pattern_a = repository
        .find_pattern(&listing_source_id, &domain_a)
        .await
        .unwrap()
        .unwrap();
    let pattern_b = repository
        .find_pattern(&listing_source_id, &domain_b)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pattern_a.url_pattern.as_deref(), Some(r"/products/"));
    assert_eq!(pattern_b.url_pattern.as_deref(), Some(r"/objects/"));
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_pattern_write_for_domain_owned_by_another_listing_source() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let owner = ListingSourceId::new();
    let other = ListingSourceId::new();
    insert_source(&pool, owner).await;
    insert_source(&pool, other).await;
    let domain_id = insert_domain(&pool, owner, "owned.example.com").await;

    let result = repository
        .save_pattern(&other, &domain_id, Some(r"/items/"))
        .await;
    assert!(matches!(result, Err(sqlx::Error::RowNotFound)));
    let stored: Option<String> =
        sqlx::query_scalar("SELECT url_pattern FROM listing_source_domains WHERE domain_id = $1")
            .bind(domain_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(stored.is_none());
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_crawl_mark_for_domain_owned_by_another_listing_source() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let owner = ListingSourceId::new();
    let other = ListingSourceId::new();
    insert_source(&pool, owner).await;
    insert_source(&pool, other).await;
    let domain_id = insert_domain(&pool, owner, "marked.example.com").await;

    let result = repository.mark_as_crawled(&other, &domain_id).await;
    assert!(matches!(result, Err(sqlx::Error::RowNotFound)));
    let crawled: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT last_crawled FROM listing_source_domains WHERE domain_id = $1")
            .bind(domain_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(crawled.is_none());
}
