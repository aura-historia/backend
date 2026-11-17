//! The one-off data migrations that come with keeping captured identities: releasing the URLs stuck
//! in an identity conflict (#2983) and retiring spellings of one product that were both captured
//! under one identity before a batch claimed the identities it captured, by the same-product rule
//! the scraper applies (`crawler::scraper::same_product`).

use crawler::CrawlRoot;
use crawler::scraper::same_product::{ProductUrlIdentity, same_product_evidence};
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
    QualificationGate, ScopedListingSource,
};
use crawler::spider::classification::url_metadata::UrlClass;
use crawler::spider::classification::url_metadata_repository::{
    UrlMetadataRepository, UrlMetadataRepositoryImpl,
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use sqlx::PgPool;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");
const ROOT: &str = "https://www.shop.example.com/";
const RELEASE_CONFLICTS: &str =
    include_str!("../migrations/20261117100001_release_identity_conflicts.sql");
const RETIRE_DUPLICATES: &str =
    include_str!("../migrations/20261117100002_retire_duplicate_identity_urls.sql");
const SAME_PRODUCT_FIXTURES: &str = include_str!("fixtures/same_product.json");

async fn insert_listing_source(pool: &PgPool) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    ListingSourceRegistrationRepositoryImpl::new(pool.clone(), QualificationGate::Off)
        .apply_snapshot(&[ScopedListingSource {
            listing_source_id,
            listing_source_name: ListingSourceName::try_from("Identity shop").unwrap(),
            listing_source_slug: ListingSourceSlugId::raw("identity-shop").unwrap(),
            crawl_enabled: true,
            crawl_root: Some(CrawlRoot::from_presentation_url(ROOT).unwrap()),
        }])
        .await
        .unwrap();
    listing_source_id
}

/// Stores a captured product URL of `listing_source_id` under `identity`, created `age_days` ago.
async fn insert_captured_url(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    path: &str,
    identity: &str,
    age_days: i32,
) -> String {
    let url = format!("{ROOT}{path}");
    UrlMetadataRepositoryImpl::new(pool.clone())
        .upsert_link(
            &listing_source_id,
            &url::Url::parse(&url).unwrap(),
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE listing_source_urls \
         SET last_source_listing_id = $2, last_extracted_source_listing_id = $2, \
             created = NOW() - make_interval(days => $3) \
         WHERE url = $1",
    )
    .bind(&url)
    .bind(identity)
    .bind(age_days)
    .execute(pool)
    .await
    .unwrap();
    url
}

async fn run(pool: &PgPool, migration: &'static str) {
    sqlx::raw_sql(migration).execute(pool).await.unwrap();
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_release_conflicted_urls_and_record_the_page_identity_of_the_others() {
    let pool = get_postgres_client().await;
    let listing_source_id = insert_listing_source(&pool).await;
    let captured =
        insert_captured_url(&pool, listing_source_id, "products/chair", "SKU-1", 1).await;
    let conflicted =
        insert_captured_url(&pool, listing_source_id, "products/tea-bowl", "J054", 1).await;
    sqlx::query(
        "UPDATE listing_source_urls \
         SET last_error_kind = 'SourceListingIdConflict', next_retry_at = NOW() + INTERVAL '4 hours' \
         WHERE url = $1",
    )
    .bind(&conflicted)
    .execute(&pool)
    .await
    .unwrap();

    run(&pool, RELEASE_CONFLICTS).await;
    run(&pool, RELEASE_CONFLICTS).await;

    let state = |url: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (Option<String>, bool)>(
                "SELECT last_extracted_source_listing_id, next_retry_at IS NULL \
                 FROM listing_source_urls WHERE url = $1",
            )
            .bind(url)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(state(captured).await, (Some("SKU-1".to_owned()), true));
    assert_eq!(state(conflicted).await, (None, true));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
/// Loveday's `table`, `table-1` and `table-2` spellings, captured without titles, show one
/// product; the earliest row keeps capturing it.
async fn should_retire_every_later_spelling_captured_under_an_identity_and_keep_the_first() {
    let pool = get_postgres_client().await;
    let listing_source_id = insert_listing_source(&pool).await;
    let other_source_id = ListingSourceId::new();
    let first =
        insert_captured_url(&pool, listing_source_id, "product/223/table/", "#2798", 3).await;
    let second =
        insert_captured_url(&pool, listing_source_id, "product/223/table-1/", "#2798", 2).await;
    let third =
        insert_captured_url(&pool, listing_source_id, "product/223/table-2/", "#2798", 1).await;
    let alone =
        insert_captured_url(&pool, listing_source_id, "product/298/lantern/", "#2866", 1).await;
    ListingSourceRegistrationRepositoryImpl::new(pool.clone(), QualificationGate::Off)
        .apply_snapshot(&[ScopedListingSource {
            listing_source_id: other_source_id,
            listing_source_name: ListingSourceName::try_from("Other shop").unwrap(),
            listing_source_slug: ListingSourceSlugId::raw("other-shop").unwrap(),
            crawl_enabled: true,
            crawl_root: Some(
                CrawlRoot::from_presentation_url("https://other.example.com/").unwrap(),
            ),
        }])
        .await
        .unwrap();
    let foreign = "https://other.example.com/product/223/table/".to_owned();
    UrlMetadataRepositoryImpl::new(pool.clone())
        .upsert_link(
            &other_source_id,
            &url::Url::parse(&foreign).unwrap(),
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE listing_source_urls \
         SET last_source_listing_id = '#2798', last_extracted_source_listing_id = '#2798' \
         WHERE url = $1",
    )
    .bind(&foreign)
    .execute(&pool)
    .await
    .unwrap();

    run(&pool, RETIRE_DUPLICATES).await;
    run(&pool, RETIRE_DUPLICATES).await;

    let disposition = |url: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT crawler_disposition FROM listing_source_urls WHERE url = $1",
            )
            .bind(url)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(disposition(first).await, "ACTIVE");
    assert_eq!(disposition(second).await, "DUPLICATE");
    assert_eq!(disposition(third).await, "DUPLICATE");
    assert_eq!(disposition(alone).await, "ACTIVE");
    assert_eq!(disposition(foreign).await, "ACTIVE");
}

#[derive(serde::Deserialize)]
struct FixtureRow {
    path: String,
    captured: String,
    page_identity: String,
    title: Option<String>,
    last_error_kind: Option<String>,
    last_scraped_days_ago: Option<i32>,
    created_days_ago: i32,
    expected: String,
}

#[derive(serde::Deserialize)]
struct FixtureGroup {
    name: String,
    rows: Vec<FixtureRow>,
}

#[derive(serde::Deserialize)]
struct Fixtures {
    groups: Vec<FixtureGroup>,
}

/// The migration and the scraper's same-product rule answer every row of the shared fixtures
/// alike: spellings, translations and untitled variants are retired, distinct products under a
/// reused stock code (HaresFur's `C327`) are kept, and the healthiest row survives a failing or
/// stale earlier one.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_retire_exactly_the_urls_the_same_product_rule_retires() {
    let pool = get_postgres_client().await;
    let listing_source_id = insert_listing_source(&pool).await;
    let fixtures: Fixtures = serde_json::from_str(SAME_PRODUCT_FIXTURES).unwrap();
    for row in fixtures.groups.iter().flat_map(|group| &group.rows) {
        let url = format!("{ROOT}{}", row.path);
        sqlx::query(
            "INSERT INTO listing_source_urls \
             (listing_source_id, url, url_class, raw_source_record_key, last_source_listing_id, \
              last_extracted_source_listing_id, last_captured_title, last_error_kind, last_scraped, \
              created) \
             VALUES ($1, $2, 'product', $2, $3, $4, $5, $6, \
                     NOW() - make_interval(days => $7), NOW() - make_interval(days => $8))",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(&url)
        .bind(&row.captured)
        .bind(&row.page_identity)
        .bind(&row.title)
        .bind(&row.last_error_kind)
        .bind(row.last_scraped_days_ago)
        .bind(row.created_days_ago)
        .execute(&pool)
        .await
        .unwrap();
    }

    run(&pool, RETIRE_DUPLICATES).await;
    run(&pool, RETIRE_DUPLICATES).await;

    let root = url::Url::parse(ROOT).unwrap();
    for group in &fixtures.groups {
        for row in &group.rows {
            let url = format!("{ROOT}{}", row.path);
            let disposition: String = sqlx::query_scalar(
                "SELECT crawler_disposition FROM listing_source_urls WHERE url = $1",
            )
            .bind(&url)
            .fetch_one(&pool)
            .await
            .unwrap();
            let survivor = group
                .rows
                .iter()
                .find(|carrier| {
                    carrier.expected == "survivor" && carrier.captured == row.page_identity
                })
                .unwrap();
            let rule_retires = row.expected != "survivor" && {
                let page_url = root.join(&row.path).unwrap();
                let carrier_url = root.join(&survivor.path).unwrap();
                same_product_evidence(
                    ProductUrlIdentity {
                        url: &page_url,
                        captured_identity: Some(&row.captured),
                        title: row.title.as_deref(),
                    },
                    &row.page_identity,
                    ProductUrlIdentity {
                        url: &carrier_url,
                        captured_identity: Some(&survivor.captured),
                        title: survivor.title.as_deref(),
                    },
                )
                .is_some()
            };

            assert_eq!(
                disposition == "DUPLICATE",
                rule_retires,
                "{}: {} ({disposition})",
                group.name,
                row.path
            );
            assert_eq!(
                rule_retires,
                row.expected == "retire",
                "{}: {}",
                group.name,
                row.path
            );
        }
    }
}
