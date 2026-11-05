use crawler::spider::classification::url_pattern_repository::{
    ListingSourceUrlPatternRepository, ListingSourceUrlPatternRepositoryImpl, UrlPatternState,
};
use crawler::spider::crawled_root::{CrawlOutcomeWrite, CrawledRoot};
use listing_source_core::ListingSourceId;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

/// A crawl-enabled ListingSource crawled from `https://{crawl_host}/`.
async fn insert_source(pool: &sqlx::PgPool, listing_source_id: ListingSourceId, crawl_host: &str) {
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, crawl_root_url, crawl_host) \
         VALUES ($1, 'Test source', 'test-source', TRUE, 'https://' || $2 || '/', $2)",
    )
    .bind(listing_source_id.as_uuid())
    .bind(crawl_host)
    .execute(pool)
    .await
    .unwrap();
}

async fn stored_pattern(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) -> Option<String> {
    sqlx::query_scalar("SELECT url_pattern FROM listing_sources WHERE listing_source_id = $1")
        .bind(listing_source_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn last_crawled(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
) -> Option<time::OffsetDateTime> {
    sqlx::query_scalar("SELECT last_crawled FROM listing_sources WHERE listing_source_id = $1")
        .bind(listing_source_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_return_none_when_listing_source_is_missing() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    insert_source(&pool, ListingSourceId::new(), "present.example.com").await;

    let found = repository
        .find_pattern(&ListingSourceId::new())
        .await
        .unwrap();

    assert!(found.is_none());
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_save_replace_and_clear_pattern_of_listing_source() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id, "patterns.example.com").await;

    let initial = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial.listing_source_id, listing_source_id);
    assert!(initial.url_pattern.is_none());
    assert_eq!(initial.url_pattern_state, UrlPatternState::Unknown);

    repository
        .save_pattern(&listing_source_id, Some(r"/products/"))
        .await
        .unwrap();
    let saved = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.url_pattern.as_deref(), Some(r"/products/"));
    assert_eq!(saved.url_pattern_state, UrlPatternState::Matched);

    repository
        .save_pattern(&listing_source_id, Some(r"/objects/"))
        .await
        .unwrap();
    let replaced = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replaced.url_pattern.as_deref(), Some(r"/objects/"));
    assert_eq!(replaced.url_pattern_state, UrlPatternState::Matched);

    repository
        .save_pattern(&listing_source_id, None)
        .await
        .unwrap();
    let cleared = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();
    assert!(cleared.url_pattern.is_none());
    assert_eq!(cleared.url_pattern_state, UrlPatternState::Unknown);

    repository
        .save_no_pattern(&listing_source_id)
        .await
        .unwrap();
    let no_pattern = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();
    assert!(no_pattern.url_pattern.is_none());
    assert_eq!(no_pattern.url_pattern_state, UrlPatternState::NoPattern);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_store_patterns_and_crawl_marks_independently_per_listing_source() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let source_a = ListingSourceId::new();
    let source_b = ListingSourceId::new();
    insert_source(&pool, source_a, "a.example.com").await;
    insert_source(&pool, source_b, "b.example.com").await;

    repository
        .save_pattern(&source_a, Some(r"/products/"))
        .await
        .unwrap();
    repository
        .save_pattern(&source_b, Some(r"/objects/"))
        .await
        .unwrap();

    let pattern_a = repository.find_pattern(&source_a).await.unwrap().unwrap();
    let pattern_b = repository.find_pattern(&source_b).await.unwrap().unwrap();
    assert_eq!(pattern_a.url_pattern.as_deref(), Some(r"/products/"));
    assert_eq!(pattern_a.url_pattern_state, UrlPatternState::Matched);
    assert!(pattern_a.last_crawled.is_none());
    assert_eq!(pattern_b.url_pattern.as_deref(), Some(r"/objects/"));
    assert_eq!(pattern_b.url_pattern_state, UrlPatternState::Matched);
    assert!(pattern_b.last_crawled.is_none());

    assert_eq!(
        repository
            .mark_as_crawled(&CrawledRoot::new(source_a, "https://a.example.com/"))
            .await
            .unwrap(),
        CrawlOutcomeWrite::Recorded
    );
    let crawled_a = repository.find_pattern(&source_a).await.unwrap().unwrap();
    let untouched_b = repository.find_pattern(&source_b).await.unwrap().unwrap();
    assert!(crawled_a.last_crawled.is_some());
    assert_eq!(crawled_a.url_pattern.as_deref(), Some(r"/products/"));
    assert!(untouched_b.last_crawled.is_none());
    assert_eq!(untouched_b.url_pattern.as_deref(), Some(r"/objects/"));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_reject_pattern_writes_for_missing_listing_source() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let owner = ListingSourceId::new();
    let missing = ListingSourceId::new();
    insert_source(&pool, owner, "owned.example.com").await;

    let result = repository.save_pattern(&missing, Some(r"/items/")).await;
    assert!(
        matches!(result, Err(sqlx::Error::RowNotFound)),
        "{result:?}"
    );
    let result = repository.save_no_pattern(&missing).await;
    assert!(
        matches!(result, Err(sqlx::Error::RowNotFound)),
        "{result:?}"
    );
    assert!(stored_pattern(&pool, owner).await.is_none());
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_reject_crawl_mark_for_missing_listing_source() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let owner = ListingSourceId::new();
    let missing = ListingSourceId::new();
    insert_source(&pool, owner, "marked.example.com").await;

    let result = repository
        .mark_as_crawled(&CrawledRoot::new(missing, "https://marked.example.com/"))
        .await;
    assert!(
        matches!(result, Err(sqlx::Error::RowNotFound)),
        "{result:?}"
    );
    assert!(last_crawled(&pool, owner).await.is_none());
}
