use crawler::CrawlerDomainId;
use crawler::spider::classification::url_metadata::{
    CrawlerDisposition, CrawlerUrlWriteOutcome, UrlClass,
};
use crawler::spider::classification::url_metadata_repository::{
    DiscoveredUrl, UrlMetadataRepository, UrlMetadataRepositoryError, UrlMetadataRepositoryImpl,
};
use listing_source_core::ListingSourceId;
use test_api::*;
use url::Url;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

fn discovered(urls: &[Url], classes: &[UrlClass]) -> Vec<DiscoveredUrl> {
    assert_eq!(urls.len(), classes.len());
    urls.iter()
        .zip(classes)
        .map(|(url, class)| DiscoveredUrl {
            url: url.clone(),
            url_class: *class,
            canonical: None,
        })
        .collect()
}

async fn insert_source(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) {
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled) \
         VALUES ($1, 'Test source', 'test-source', TRUE)",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_domain(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
    domain: &str,
) -> CrawlerDomainId {
    let domain_id = CrawlerDomainId::new();
    sqlx::query(
        "INSERT INTO listing_source_domains (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, $3, $3)",
    )
    .bind(domain_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .bind(domain)
    .execute(pool)
    .await
    .unwrap();
    domain_id
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_insert_and_update_url_for_its_same_owner() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "same-owner.example.com").await;
    let url = Url::parse("https://same-owner.example.com/products/1").unwrap();

    let inserted = repository
        .upsert_link(
            &listing_source_id,
            &domain_id,
            &url,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();
    assert_eq!(inserted.listing_source_id, listing_source_id);
    assert_eq!(inserted.domain_id, domain_id);
    assert_eq!(inserted.url, url);
    assert_eq!(inserted.url_class, UrlClass::ProductListing);
    assert_eq!(inserted.disposition, CrawlerDisposition::Active);
    assert!(inserted.last_scraped.is_none());
    assert!(inserted.last_scraped_hash.is_none());

    let updated = repository
        .upsert_link(&listing_source_id, &domain_id, &url, &UrlClass::Other)
        .await
        .unwrap();
    assert_eq!(updated.listing_source_id, listing_source_id);
    assert_eq!(updated.domain_id, domain_id);
    assert_eq!(updated.url, url);
    assert_eq!(updated.url_class, UrlClass::Other);
    assert_eq!(updated.disposition, CrawlerDisposition::Active);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_transition_owned_url_to_dormant_without_reactivation() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "presence.example.com").await;
    let url = Url::parse("https://presence.example.com/products/1").unwrap();
    repository
        .upsert_link(
            &listing_source_id,
            &domain_id,
            &url,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();

    assert_eq!(
        repository
            .set_disposition(&listing_source_id, &url, CrawlerDisposition::DormantSold)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    assert_eq!(
        repository
            .set_disposition(&listing_source_id, &url, CrawlerDisposition::Active)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );
    let row: (String, uuid::Uuid) = sqlx::query_as(
        "SELECT crawler_disposition, domain_id FROM listing_source_urls \
         WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, CrawlerDisposition::DormantSold.as_str());
    assert_eq!(row.1, *domain_id.as_uuid());
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_dormant_sold_url_absorbing_during_rediscovery_and_active_update() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "absorbing.example.com").await;
    let url = Url::parse("https://absorbing.example.com/products/1").unwrap();
    repository
        .upsert_link(
            &listing_source_id,
            &domain_id,
            &url,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();

    assert_eq!(
        repository
            .set_disposition(&listing_source_id, &url, CrawlerDisposition::DormantSold)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let rediscovered = repository
        .upsert_link(&listing_source_id, &domain_id, &url, &UrlClass::Other)
        .await
        .unwrap();
    assert_eq!(rediscovered.disposition, CrawlerDisposition::DormantSold);
    assert_eq!(rediscovered.url_class, UrlClass::ProductListing);

    assert_eq!(
        repository
            .set_disposition(&listing_source_id, &url, CrawlerDisposition::Active)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );
    let disposition: String = sqlx::query_scalar(
        "SELECT crawler_disposition FROM listing_source_urls \
         WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(disposition, CrawlerDisposition::DormantSold.as_str());
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_upsert_valid_url_batch_for_one_owned_domain() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "batch.example.com").await;
    let first = Url::parse("https://batch.example.com/products/1").unwrap();
    let second = Url::parse("https://batch.example.com/products/2").unwrap();

    let records = repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            &discovered(
                &[first.clone(), second.clone()],
                &[UrlClass::ProductListing, UrlClass::Category],
            ),
        )
        .await
        .unwrap();

    assert_eq!(records.len(), 2);
    assert!(records.iter().any(|record| {
        record.listing_source_id == listing_source_id
            && record.domain_id == domain_id
            && record.url == first
            && record.url_class == UrlClass::ProductListing
            && record.disposition == CrawlerDisposition::Active
    }));
    assert!(records.iter().any(|record| {
        record.listing_source_id == listing_source_id
            && record.domain_id == domain_id
            && record.url == second
            && record.url_class == UrlClass::Category
            && record.disposition == CrawlerDisposition::Active
    }));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_reject_duplicate_urls_in_batch_without_persisting_any_row() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "duplicates.example.com").await;
    let url = Url::parse("https://duplicates.example.com/products/1").unwrap();

    let result = repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            &discovered(
                &[url.clone(), url.clone()],
                &[UrlClass::ProductListing, UrlClass::Other],
            ),
        )
        .await;

    assert!(matches!(
        result,
        Err(UrlMetadataRepositoryError::Database {
            source: sqlx::Error::Database(_)
        })
    ));
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_delete_urls_when_their_domain_is_deleted() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "cascade.example.com").await;
    let url = Url::parse("https://cascade.example.com/products/1").unwrap();
    repository
        .upsert_link(
            &listing_source_id,
            &domain_id,
            &url,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();

    sqlx::query(
        "DELETE FROM listing_source_domains WHERE listing_source_id = $1 AND domain_id = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(domain_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM listing_source_urls WHERE url = $1")
        .bind(url.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
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

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_reject_cross_source_url_claim_when_requested_domain_does_not_own_the_url_host() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let source_a = ListingSourceId::new();
    let source_b = ListingSourceId::new();
    insert_source(&pool, source_a).await;
    insert_source(&pool, source_b).await;
    let domain_a = insert_domain(&pool, source_a, "shared.example.com").await;
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
        Err(UrlMetadataRepositoryError::UrlHostDoesNotMatchDomain { .. })
    ));

    let row: (uuid::Uuid, uuid::Uuid, String) = sqlx::query_as(
        "SELECT listing_source_id, domain_id, url_class FROM listing_source_urls WHERE url = $1",
    )
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, *source_a.as_uuid());
    assert_eq!(row.1, *domain_a.as_uuid());
    assert_eq!(row.2, "product");
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_reject_duplicate_canonical_domain_for_same_source() {
    let pool = get_postgres_client().await;
    let source = ListingSourceId::new();
    insert_source(&pool, source).await;
    insert_domain(&pool, source, "example.com").await;

    let duplicate_domain_id = CrawlerDomainId::new();
    let duplicate = sqlx::query(
        "INSERT INTO listing_source_domains \
         (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(duplicate_domain_id.as_uuid())
    .bind(source.as_uuid())
    .bind("example.com")
    .bind("www.example.com")
    .execute(&pool)
    .await;

    assert!(duplicate.is_err());
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_roll_back_batch_when_one_url_is_owned_by_another_listing_source() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let source_a = ListingSourceId::new();
    let source_b = ListingSourceId::new();
    insert_source(&pool, source_a).await;
    insert_source(&pool, source_b).await;
    let domain_a = insert_domain(&pool, source_a, "shared.example.com").await;
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
            &discovered(
                &[conflicting.clone(), fresh.clone()],
                &[UrlClass::Other, UrlClass::ProductListing],
            ),
        )
        .await;
    assert!(matches!(
        result,
        Err(UrlMetadataRepositoryError::UrlHostDoesNotMatchDomain { .. })
    ));
    let fresh_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM listing_source_urls WHERE url = $1")
            .bind(fresh.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(fresh_count, 0);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_accept_bare_and_www_domain_equivalence_but_reject_other_subdomains() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let source = ListingSourceId::new();
    insert_source(&pool, source).await;
    let domain = insert_domain(&pool, source, "example.com").await;

    repository
        .upsert_link(
            &source,
            &domain,
            &Url::parse("https://www.example.com/products/1").unwrap(),
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();
    let result = repository
        .upsert_link(
            &source,
            &domain,
            &Url::parse("https://catalog.example.com/products/2").unwrap(),
            &UrlClass::ProductListing,
        )
        .await;

    assert!(matches!(
        result,
        Err(UrlMetadataRepositoryError::UrlHostDoesNotMatchDomain { .. })
    ));
}

async fn insert_scraped_url(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
    domain_id: CrawlerDomainId,
    url: &str,
) {
    sqlx::query(
        "INSERT INTO listing_source_urls \
         (listing_source_id, domain_id, url, raw_source_record_key, url_class, last_scraped, last_scraped_hash, last_source_listing_id) \
         VALUES ($1, $2, $3, $3, 'product', NOW(), 'content-hash', 'sku-1')",
    )
    .bind(listing_source_id.as_uuid())
    .bind(domain_id.as_uuid())
    .bind(url)
    .execute(pool)
    .await
    .unwrap();
}

type StoredRow = (String, String, Option<String>, Option<String>);

async fn stored_rows(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) -> Vec<StoredRow> {
    sqlx::query_as(
        "SELECT url, raw_source_record_key, last_scraped_hash, last_source_listing_id \
         FROM listing_source_urls WHERE listing_source_id = $1 ORDER BY url COLLATE \"C\"",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_all(pool)
    .await
    .unwrap()
}

fn scraped_row(url: &str, raw_source_record_key: &str) -> StoredRow {
    (
        url.to_owned(),
        raw_source_record_key.to_owned(),
        Some("content-hash".to_owned()),
        Some("sku-1".to_owned()),
    )
}

/// The spelling the crawler stored before it kept the fetched encoding, and the fetched one.
const LEGACY_SPELLING: &str = "https://spelling.example.com/collections/vendors?q=A%2C+B";
const FETCHED_SPELLING: &str = "https://spelling.example.com/collections/vendors?q=A%2C%20B";

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_move_row_stored_under_earlier_spelling_when_spider_rediscovers_fetched_url() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "spelling.example.com").await;
    insert_scraped_url(&pool, listing_source_id, domain_id, LEGACY_SPELLING).await;
    let fetched = Url::parse(FETCHED_SPELLING).unwrap();

    let records = repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            &discovered(std::slice::from_ref(&fetched), &[UrlClass::Category]),
        )
        .await
        .unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].url, fetched);
    assert_eq!(records[0].url_class, UrlClass::Category);
    assert!(records[0].last_scraped.is_some());
    assert_eq!(
        stored_rows(&pool, listing_source_id).await,
        vec![scraped_row(FETCHED_SPELLING, LEGACY_SPELLING)]
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_row_under_earlier_spelling_when_shop_links_that_spelling_too() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "spelling.example.com").await;
    insert_scraped_url(&pool, listing_source_id, domain_id, LEGACY_SPELLING).await;

    repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            &discovered(
                &[
                    Url::parse(FETCHED_SPELLING).unwrap(),
                    Url::parse(LEGACY_SPELLING).unwrap(),
                ],
                &[UrlClass::Category, UrlClass::Category],
            ),
        )
        .await
        .unwrap();

    assert_eq!(
        stored_rows(&pool, listing_source_id).await,
        vec![
            (
                FETCHED_SPELLING.to_owned(),
                FETCHED_SPELLING.to_owned(),
                None,
                None,
            ),
            scraped_row(LEGACY_SPELLING, LEGACY_SPELLING),
        ]
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_leave_row_under_earlier_spelling_when_fetched_url_already_has_a_row() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "spelling.example.com").await;
    insert_scraped_url(&pool, listing_source_id, domain_id, LEGACY_SPELLING).await;
    insert_scraped_url(&pool, listing_source_id, domain_id, FETCHED_SPELLING).await;

    repository
        .upsert_link(
            &listing_source_id,
            &domain_id,
            &Url::parse(FETCHED_SPELLING).unwrap(),
            &UrlClass::Category,
        )
        .await
        .unwrap();

    assert_eq!(
        stored_rows(&pool, listing_source_id).await,
        vec![
            scraped_row(FETCHED_SPELLING, FETCHED_SPELLING),
            scraped_row(LEGACY_SPELLING, LEGACY_SPELLING),
        ]
    );
}

/// The spelling the crawler stored while it dropped trailing slashes, and the one the shop links.
const UNSLASHED_SPELLING: &str = "https://slash.example.com/product/106/papier-mache-tray";
const SLASHED_SPELLING: &str = "https://slash.example.com/product/106/papier-mache-tray/";

async fn upsert_batch(
    repository: &UrlMetadataRepositoryImpl,
    listing_source_id: ListingSourceId,
    domain_id: CrawlerDomainId,
    urls: &[&str],
) -> Vec<String> {
    let urls = urls
        .iter()
        .map(|url| Url::parse(url).unwrap())
        .collect::<Vec<_>>();
    repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            &discovered(&urls, &vec![UrlClass::ProductListing; urls.len()]),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|record| record.url.to_string())
        .collect()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_move_row_stored_without_trailing_slash_when_spider_rediscovers_slashed_url() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "slash.example.com").await;
    insert_scraped_url(&pool, listing_source_id, domain_id, UNSLASHED_SPELLING).await;

    let records = upsert_batch(
        &repository,
        listing_source_id,
        domain_id,
        &[SLASHED_SPELLING],
    )
    .await;

    assert_eq!(records, vec![SLASHED_SPELLING.to_owned()]);
    assert_eq!(
        stored_rows(&pool, listing_source_id).await,
        vec![scraped_row(SLASHED_SPELLING, UNSLASHED_SPELLING)]
    );
}

/// A row stored while the crawler re-encoded queries and dropped the trailing slash moves to the
/// fetched spelling once no row holds the spelling without the slash.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_move_row_stored_re_encoded_without_trailing_slash_to_slashed_url() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "slash.example.com").await;
    let legacy = "https://slash.example.com/collections/vendors?q=A%2C+B";
    let fetched = "https://slash.example.com/collections/vendors/?q=A%2C%20B";
    insert_scraped_url(&pool, listing_source_id, domain_id, legacy).await;

    upsert_batch(&repository, listing_source_id, domain_id, &[fetched]).await;

    assert_eq!(
        stored_rows(&pool, listing_source_id).await,
        vec![scraped_row(fetched, legacy)]
    );
}

/// A page with rows under both earlier spellings keeps the more recent one: the row stored without
/// the slash moves, and the re-encoded one stays put, since the fetched spelling has a row by then.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_move_only_unslashed_row_when_re_encoded_row_of_the_page_exists_too() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "slash.example.com").await;
    let re_encoded = "https://slash.example.com/collections/vendors?q=A%2C+B";
    let unslashed = "https://slash.example.com/collections/vendors?q=A%2C%20B";
    let fetched = "https://slash.example.com/collections/vendors/?q=A%2C%20B";
    insert_scraped_url(&pool, listing_source_id, domain_id, re_encoded).await;
    insert_scraped_url(&pool, listing_source_id, domain_id, unslashed).await;

    let records = upsert_batch(&repository, listing_source_id, domain_id, &[fetched]).await;

    assert_eq!(records, vec![fetched.to_owned()]);
    assert_eq!(
        stored_rows(&pool, listing_source_id).await,
        vec![
            scraped_row(fetched, unslashed),
            scraped_row(re_encoded, re_encoded),
        ]
    );
}

/// A shop that links a page with and without the trailing slash keeps one row under the slashed
/// spelling, whether both arrive in one batch or the unslashed one arrives after the slashed one
/// was stored, and a row stored without the slash moves to it.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_one_row_under_slashed_spelling_when_shop_links_both_spellings() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "slash.example.com").await;
    insert_scraped_url(&pool, listing_source_id, domain_id, UNSLASHED_SPELLING).await;

    let both = upsert_batch(
        &repository,
        listing_source_id,
        domain_id,
        &[UNSLASHED_SPELLING, SLASHED_SPELLING],
    )
    .await;
    let unslashed_later = upsert_batch(
        &repository,
        listing_source_id,
        domain_id,
        &[UNSLASHED_SPELLING],
    )
    .await;

    assert_eq!(both, vec![SLASHED_SPELLING.to_owned()]);
    assert_eq!(unslashed_later, vec![SLASHED_SPELLING.to_owned()]);
    assert_eq!(
        stored_rows(&pool, listing_source_id).await,
        vec![scraped_row(SLASHED_SPELLING, UNSLASHED_SPELLING)]
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_store_both_spellings_once_under_slashed_one_when_page_is_new() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "slash.example.com").await;

    let records = upsert_batch(
        &repository,
        listing_source_id,
        domain_id,
        &[UNSLASHED_SPELLING, SLASHED_SPELLING],
    )
    .await;

    assert_eq!(records, vec![SLASHED_SPELLING.to_owned()]);
    assert_eq!(
        stored_rows(&pool, listing_source_id).await,
        vec![(
            SLASHED_SPELLING.to_owned(),
            SLASHED_SPELLING.to_owned(),
            None,
            None
        )]
    );
}

const SHOP: &str = "https://canonical.example.com";

fn shop_url(path: &str) -> Url {
    Url::parse(&format!("{SHOP}{path}")).unwrap()
}

fn product(path: &str, canonical: Option<&str>) -> DiscoveredUrl {
    DiscoveredUrl {
        url: shop_url(path),
        url_class: UrlClass::ProductListing,
        canonical: canonical.map(shop_url),
    }
}

async fn dispositions(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT url, crawler_disposition FROM listing_source_urls \
         WHERE listing_source_id = $1 ORDER BY url COLLATE \"C\"",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn canonical_shop(pool: &sqlx::PgPool) -> (ListingSourceId, CrawlerDomainId) {
    let listing_source_id = ListingSourceId::new();
    insert_source(pool, listing_source_id).await;
    let domain_id = insert_domain(pool, listing_source_id, "canonical.example.com").await;
    (listing_source_id, domain_id)
}

/// A Shopify shop links each product under its collections too. The page is stored under its
/// canonical `/products/` URL, and a row an earlier crawl stored under the collection URL moves
/// there with its scrape state and raw-capture identity.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_move_a_shopify_collection_url_row_to_its_canonical_product_url() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let (listing_source_id, domain_id) = canonical_shop(&pool).await;
    let collection_url = format!("{SHOP}/collections/chairs/products/oak-chair");
    insert_scraped_url(&pool, listing_source_id, domain_id, &collection_url).await;

    let stored = repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            &[product(
                "/collections/chairs/products/oak-chair",
                Some("/products/oak-chair"),
            )],
        )
        .await
        .unwrap();

    assert_eq!(
        stored
            .iter()
            .map(|row| row.url.to_string())
            .collect::<Vec<_>>(),
        vec![format!("{SHOP}/products/oak-chair")]
    );
    assert_eq!(
        stored_rows(&pool, listing_source_id).await,
        vec![scraped_row(
            &format!("{SHOP}/products/oak-chair"),
            &collection_url
        )]
    );
}

/// When the canonical URL already has a row, the duplicate's row is retired as `DUPLICATE`, which
/// the scraper never selects, and a crawl that finds the duplicate again leaves it retired.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_retire_a_duplicate_row_when_its_canonical_url_is_stored() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let (listing_source_id, domain_id) = canonical_shop(&pool).await;
    for path in [
        "/collections/chairs/products/oak-chair",
        "/products/oak-chair/",
    ] {
        insert_scraped_url(
            &pool,
            listing_source_id,
            domain_id,
            &format!("{SHOP}{path}"),
        )
        .await;
    }

    repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            &[product(
                "/collections/chairs/products/oak-chair",
                Some("/products/oak-chair"),
            )],
        )
        .await
        .unwrap();
    let rediscovered = repository
        .upsert_link(
            &listing_source_id,
            &domain_id,
            &shop_url("/collections/chairs/products/oak-chair"),
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();

    assert_eq!(rediscovered.disposition, CrawlerDisposition::Duplicate);
    assert_eq!(
        dispositions(&pool, listing_source_id).await,
        vec![
            (
                format!("{SHOP}/collections/chairs/products/oak-chair"),
                "DUPLICATE".to_owned()
            ),
            (format!("{SHOP}/products/oak-chair/"), "ACTIVE".to_owned()),
        ],
        "the canonical page keeps its row under its stored spelling"
    );
}

/// A language copy and Shopify collection copies found in one batch with their canonical page
/// collapse to one row, the canonical page's; the first existing duplicate row moves there and the
/// others are retired.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_collapse_duplicates_of_one_page_in_a_batch_onto_one_row() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let (listing_source_id, domain_id) = canonical_shop(&pool).await;
    for path in ["/collections/a/products/bowl", "/en/products/bowl"] {
        insert_scraped_url(
            &pool,
            listing_source_id,
            domain_id,
            &format!("{SHOP}{path}"),
        )
        .await;
    }

    let stored = repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            &[
                product("/en/products/bowl", Some("/products/bowl")),
                product("/collections/a/products/bowl", Some("/products/bowl")),
                product("/products/bowl", None),
                product("/collections/b/products/bowl", Some("/products/bowl")),
            ],
        )
        .await
        .unwrap();

    assert_eq!(stored.len(), 1);
    assert_eq!(
        dispositions(&pool, listing_source_id).await,
        vec![
            (format!("{SHOP}/en/products/bowl"), "DUPLICATE".to_owned()),
            (format!("{SHOP}/products/bowl"), "ACTIVE".to_owned()),
        ]
    );
    assert_eq!(
        stored_rows(&pool, listing_source_id).await[1],
        scraped_row(
            &format!("{SHOP}/products/bowl"),
            &format!("{SHOP}/collections/a/products/bowl")
        ),
        "the first duplicate row in URL order moved to the canonical URL"
    );
}

/// A canonical URL that another ListingSource owns, that is no URL of the crawled domain, or that
/// is itself canonicalized in the batch does not move identity: the page is stored as fetched.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_store_a_page_as_fetched_when_its_canonical_url_cannot_take_it() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let (listing_source_id, domain_id) = canonical_shop(&pool).await;
    let other_source = ListingSourceId::new();
    insert_source(&pool, other_source).await;
    let other_domain = insert_domain(&pool, other_source, "other.example.com").await;
    insert_scraped_url(
        &pool,
        other_source,
        other_domain,
        &format!("{SHOP}/products/owned"),
    )
    .await;

    repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            &[
                product("/collections/a/products/owned", Some("/products/owned")),
                DiscoveredUrl {
                    canonical: Some(
                        Url::parse("https://elsewhere.example.com/products/x").unwrap(),
                    ),
                    ..product("/collections/a/products/x", None)
                },
                product(
                    "/collections/a/products/chain",
                    Some("/collections/b/products/chain"),
                ),
                product("/collections/b/products/chain", Some("/products/chain")),
            ],
        )
        .await
        .unwrap();

    assert_eq!(
        dispositions(&pool, listing_source_id)
            .await
            .into_iter()
            .map(|(url, _)| url)
            .collect::<Vec<_>>(),
        vec![
            format!("{SHOP}/collections/a/products/chain"),
            format!("{SHOP}/collections/a/products/owned"),
            format!("{SHOP}/collections/a/products/x"),
            format!("{SHOP}/collections/b/products/chain"),
        ]
    );
}
