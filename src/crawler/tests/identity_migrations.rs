//! The one-off data migration that comes with keeping captured identities: releasing the URLs stuck
//! in an identity conflict (#2983) and recording the page identity of the URLs whose last capture
//! showed a product.

use crawler::CrawlRoot;
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

/// Stores a product URL of `listing_source_id` captured under `identity` with the title `title`
/// (`None` after a captured removal).
async fn insert_captured_url(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    path: &str,
    identity: &str,
    title: Option<&str>,
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
             last_captured_title = $3 \
         WHERE url = $1",
    )
    .bind(&url)
    .bind(identity)
    .bind(title)
    .execute(pool)
    .await
    .unwrap();
    url
}

async fn run(pool: &PgPool, migration: &'static str) {
    sqlx::raw_sql(migration).execute(pool).await.unwrap();
}

/// A conflicted URL is due at once and carries nothing; a URL whose last capture showed a product
/// records its identity as the page's; one whose last capture was a removal records none.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_release_conflicted_urls_and_record_the_page_identity_of_the_others() {
    let pool = get_postgres_client().await;
    let listing_source_id = insert_listing_source(&pool).await;
    let captured = insert_captured_url(
        &pool,
        listing_source_id,
        "products/chair",
        "SKU-1",
        Some("Chair"),
    )
    .await;
    let conflicted = insert_captured_url(
        &pool,
        listing_source_id,
        "products/tea-bowl",
        "J054",
        Some("Tea bowl"),
    )
    .await;
    let removed =
        insert_captured_url(&pool, listing_source_id, "products/lamp", "SKU-7", None).await;
    sqlx::query(
        "UPDATE listing_source_urls \
         SET last_error_kind = 'SourceListingIdConflict', next_retry_at = NOW() + INTERVAL '4 hours' \
         WHERE url = $1",
    )
    .bind(&conflicted)
    .execute(&pool)
    .await
    .unwrap();
    // Before the migration no row records the identity its page showed.
    sqlx::query("UPDATE listing_source_urls SET last_extracted_source_listing_id = NULL")
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
    assert_eq!(state(removed).await, (None, true));
}
