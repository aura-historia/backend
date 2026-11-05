//! Real SQLx crawler migrations on an owned, initially empty database.

use crate::runtime::apply_local_migrations;
use test_api::{IntegrationTestService, Postgres, get_postgres_client};

pub(crate) struct CrawlerDatabase;

#[async_trait::async_trait]
impl IntegrationTestService for CrawlerDatabase {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    async fn set_up(&self) {
        // Keep the actual SQLx migrator and ledger; raw schema SQL is not equivalent.
        Postgres::new("src/crawler/tests/fixtures/empty_database")
            .set_up()
            .await;
        let pool = get_postgres_client().await;
        apply_local_migrations(&pool).await;
    }
}

/// Gives `listing_source_id` the crawl root `presentation_url` derives, as a scope refresh that
/// holds only this source does: a changed root moves the source and clears its crawl state and
/// frontier (`docs/crawler/README.md#listingsource-scope`).
pub(crate) async fn refresh_scope_with_crawl_root(
    pool: &sqlx::PgPool,
    listing_source_id: listing_source_core::ListingSourceId,
    presentation_url: &str,
) {
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
        ScopedListingSource,
    };
    let crawl_root = crate::CrawlRoot::from_presentation_url(presentation_url)
        .unwrap_or_else(|error| panic!("{presentation_url} is a crawl root: {error}"));
    ListingSourceRegistrationRepositoryImpl::new(pool.clone())
        .apply_snapshot(&[ScopedListingSource {
            listing_source_id,
            listing_source_name: listing_source_core::ListingSourceName::try_from("Test source")
                .unwrap_or_else(|error| panic!("valid test name: {error}")),
            listing_source_slug: listing_source_core::ListingSourceSlugId::raw("test-source")
                .unwrap_or_else(|error| panic!("valid test slug: {error}")),
            crawl_enabled: true,
            crawl_root: Some(crawl_root),
        }])
        .await
        .unwrap_or_else(|error| panic!("apply the scope snapshot: {error}"));
}

/// Stores the crawl root `presentation_url` derives on an existing ListingSource row, without
/// the move a scope refresh applies, for a test that only needs a crawlable source.
pub(crate) async fn store_crawl_root(
    pool: &sqlx::PgPool,
    listing_source_id: listing_source_core::ListingSourceId,
    presentation_url: &str,
) {
    let crawl_root = crate::CrawlRoot::from_presentation_url(presentation_url)
        .unwrap_or_else(|error| panic!("{presentation_url} is a crawl root: {error}"));
    sqlx::query(
        "UPDATE listing_sources SET crawl_root_url = $2, crawl_host = $3 \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .bind(crawl_root.url().as_str())
    .bind(crawl_root.host())
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("store the crawl root: {error}"));
}
