use crawler::spider::classification::url_metadata::UrlClass;
use crawler::spider::classification::url_metadata_repository::{
    UrlMetadataRepository, UrlMetadataRepositoryError, UrlMetadataRepositoryImpl,
};
use listing_source_core::ListingSourceId;
use test_api::*;
use url::Url;

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
async fn should_reject_new_url_when_domain_belongs_to_another_listing_source() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let source_a = ListingSourceId::new();
    let source_b = ListingSourceId::new();
    insert_source(&pool, source_a).await;
    insert_source(&pool, source_b).await;
    let domain_b = insert_domain(&pool, source_b, "b.example.com").await;
    let url = Url::parse("https://b.example.com/product/1").unwrap();

    let result = repository
        .upsert_link(&source_a, &domain_b, &url, &UrlClass::ProductListing)
        .await;
    assert!(matches!(
        result,
        Err(UrlMetadataRepositoryError::DomainNotOwnedByListingSource { .. })
    ));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM listing_source_urls")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_cross_source_url_conflict_without_mutating_original_row() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let source_a = ListingSourceId::new();
    let source_b = ListingSourceId::new();
    insert_source(&pool, source_a).await;
    insert_source(&pool, source_b).await;
    let domain_a = insert_domain(&pool, source_a, "a.example.com").await;
    let domain_b = insert_domain(&pool, source_b, "b.example.com").await;
    let url = Url::parse("https://shared.example.com/product/1").unwrap();

    repository
        .upsert_link(&source_a, &domain_a, &url, &UrlClass::ProductListing)
        .await
        .unwrap();
    let conflict = repository
        .upsert_link(&source_b, &domain_b, &url, &UrlClass::Other)
        .await;
    assert!(matches!(
        conflict,
        Err(UrlMetadataRepositoryError::UrlOwnedByAnotherListingSource { .. })
    ));

    let row: (uuid::Uuid, uuid::Uuid, String) = sqlx::query_as(
        "SELECT listing_source_id, domain_id, url_class FROM listing_source_urls WHERE url = $1",
    )
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, uuid::Uuid::from(source_a));
    assert_eq!(row.1, domain_a);
    assert_eq!(row.2, "product");
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_allow_same_source_url_to_move_between_its_domains() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let source = ListingSourceId::new();
    insert_source(&pool, source).await;
    let first_domain = insert_domain(&pool, source, "first.example.com").await;
    let second_domain = insert_domain(&pool, source, "second.example.com").await;
    let url = Url::parse("https://shared.example.com/product/1").unwrap();

    repository
        .upsert_link(&source, &first_domain, &url, &UrlClass::Other)
        .await
        .unwrap();
    let record = repository
        .upsert_link(&source, &second_domain, &url, &UrlClass::ProductListing)
        .await
        .unwrap();
    assert_eq!(record.listing_source_id, source);
    assert_eq!(record.domain_id, second_domain);
    assert_eq!(record.url_class, UrlClass::ProductListing);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_roll_back_batch_when_one_url_is_owned_by_another_listing_source() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let source_a = ListingSourceId::new();
    let source_b = ListingSourceId::new();
    insert_source(&pool, source_a).await;
    insert_source(&pool, source_b).await;
    let domain_a = insert_domain(&pool, source_a, "batch-a.example.com").await;
    let domain_b = insert_domain(&pool, source_b, "batch-b.example.com").await;
    let conflicting = Url::parse("https://shared.example.com/product/1").unwrap();
    let fresh = Url::parse("https://batch-b.example.com/product/2").unwrap();
    repository
        .upsert_link(
            &source_a,
            &domain_a,
            &conflicting,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();

    let result = repository
        .upsert_links_batch(
            &source_b,
            &domain_b,
            &[conflicting.clone(), fresh.clone()],
            &[UrlClass::Other, UrlClass::ProductListing],
        )
        .await;
    assert!(matches!(
        result,
        Err(UrlMetadataRepositoryError::UrlOwnedByAnotherListingSource { .. })
    ));
    let fresh_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM listing_source_urls WHERE url = $1")
            .bind(fresh.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(fresh_count, 0);
}
