//! Real crawler-schema candidate persistence tests.
//! Separate executable: do not mix this fixture with the business-schema library tests.

use crawler::CrawlerDomainId;
use crawler::scraper::candidate_service::{ScraperCandidateService, ScraperCandidateServiceImpl};
use crawler::spider::classification::url_metadata::{CrawlerDisposition, CrawlerUrlWriteOutcome};
use listing_source_core::ListingSourceId;
use sqlx::PgPool;
use test_api::IntegrationTestService;
use url::Url;

const POSTGRES: test_api::Postgres = test_api::Postgres::new("src/crawler/migrations");

async fn seed_product(pool: &PgPool) -> (ListingSourceId, Url) {
    let id = ListingSourceId::new();
    let domain_id = CrawlerDomainId::new();
    let url = Url::parse("https://custody.example.test/products/one").unwrap();
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled) VALUES ($1, 'Custody fixture', 'custody-fixture', TRUE)")
        .bind(id.as_uuid()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO listing_source_domains (domain_id, listing_source_id, listing_source_domain, crawl_root_host) VALUES ($1, $2, 'custody.example.test', 'custody.example.test')")
        .bind(domain_id.as_uuid()).bind(id.as_uuid()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO listing_source_urls (listing_source_id, domain_id, url, url_class, last_scraped, last_scraped_hash, last_scraped_schema_fingerprint, last_captured_raw_input_sha256) VALUES ($1, $2, $3, 'product', NOW() - INTERVAL '2 days', 'same-page', 'same-schema', $4)")
        .bind(id.as_uuid()).bind(domain_id.as_uuid()).bind(url.as_str()).bind(vec![3_u8; 32]).execute(pool).await.unwrap();
    (id, url)
}

#[serial_test::serial]
#[test_api::aura_integration_test(services = [POSTGRES])]
async fn should_clear_product_fingerprints_on_removal_and_fence_stale_removal_after_restore() {
    let pool = test_api::get_postgres_client().await;
    let (id, url) = seed_product(&pool).await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    // This test persists a supplied, valid-sized removal hash; hash construction is tested
    // separately with the private raw-input implementation.
    let removal = [7_u8; 32];
    assert_eq!(
        service
            .mark_removed(&id, &url, &removal[..], Some(&[3_u8; 32]))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );
    let (page, schema, hash, disposition, scraped): (Option<String>, Option<String>, Vec<u8>, String, bool) = sqlx::query_as("SELECT last_scraped_hash, last_scraped_schema_fingerprint, last_captured_raw_input_sha256, crawler_disposition, last_scraped IS NOT NULL FROM listing_source_urls WHERE url = $1")
        .bind(url.as_str()).fetch_one(&pool).await.unwrap();
    assert_eq!((page, schema), (None, None));
    assert_eq!(hash, &removal[..]);
    assert_eq!(disposition, "ACTIVE");
    assert!(scraped);

    sqlx::query(
        "UPDATE listing_source_urls SET last_scraped = NOW() - INTERVAL '2 days' WHERE url = $1",
    )
    .bind(url.as_str())
    .execute(&pool)
    .await
    .unwrap();
    let candidates = service.get_candidates(1, 1, &[]).await.unwrap();
    assert_eq!(
        candidates.len(),
        1,
        "removed URL stays eligible for later recheck"
    );
    assert!(candidates[0].last_scraped_hash.is_none());
    assert!(candidates[0].last_scraped_schema_fingerprint.is_none());
    assert_eq!(
        service
            .mark_as_scraped(
                &id,
                &url,
                "same-page",
                "same-schema",
                &[3_u8; 32],
                CrawlerDisposition::Active,
                Some(&removal[..])
            )
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );
    assert_eq!(
        service
            .mark_removed(&id, &url, &removal[..], Some(&removal[..]))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );
    let (page, schema, hash): (String, String, Vec<u8>) = sqlx::query_as("SELECT last_scraped_hash, last_scraped_schema_fingerprint, last_captured_raw_input_sha256 FROM listing_source_urls WHERE url = $1")
        .bind(url.as_str()).fetch_one(&pool).await.unwrap();
    assert_eq!(page, "same-page");
    assert_eq!(schema, "same-schema");
    assert_eq!(hash, vec![3; 32]);
}

#[serial_test::serial]
#[test_api::aura_integration_test(services = [POSTGRES])]
async fn should_leave_due_progress_unchanged_when_local_mark_statement_fails() {
    let pool = test_api::get_postgres_client().await;
    let (id, url) = seed_product(&pool).await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    // Existing byte-length constraint injects a statement failure; no DDL or trigger.
    assert!(
        service
            .mark_as_scraped(
                &id,
                &url,
                "new-page",
                "new-schema",
                &[9; 31],
                CrawlerDisposition::DormantSold,
                Some(&[3; 32])
            )
            .await
            .is_err()
    );
    let candidates = service.get_candidates(1, 1, &[]).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].last_scraped_hash.as_deref(),
        Some("same-page")
    );
    assert_eq!(
        candidates[0].last_scraped_schema_fingerprint.as_deref(),
        Some("same-schema")
    );
    assert_eq!(
        candidates[0].last_captured_raw_input_sha256.as_deref(),
        Some(&[3; 32][..])
    );
    assert_eq!(
        service
            .mark_as_scraped(
                &id,
                &url,
                "new-page",
                "new-schema",
                &[9; 32],
                CrawlerDisposition::DormantSold,
                Some(&[3; 32])
            )
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );
    assert!(service.get_candidates(1, 1, &[]).await.unwrap().is_empty());
}
