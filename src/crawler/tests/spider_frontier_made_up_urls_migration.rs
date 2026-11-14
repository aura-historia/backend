//! The one-off migration that drops frontier URLs made up by resolving found links against the
//! page URL instead of the page's `<base href>` (#2956): it drops the URLs that repeat their first
//! path segment further down the path and keeps every other frontier URL.

use crawler::CrawlRoot;
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
    ScopedListingSource,
};
use crawler::spider::crawl_frontier_repository::{
    CrawlFrontierRepository, CrawlFrontierRepositoryImpl,
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use sqlx::PgPool;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");
const ROOT: &str = "https://www.shop.example.com/";
const MIGRATION: &str = include_str!("../migrations/20261114000000_drop_made_up_frontier_urls.sql");

async fn insert_listing_source(pool: &PgPool) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    ListingSourceRegistrationRepositoryImpl::new(pool.clone())
        .apply_snapshot(&[ScopedListingSource {
            listing_source_id,
            listing_source_name: ListingSourceName::try_from("Base href shop").unwrap(),
            listing_source_slug: ListingSourceSlugId::raw("base-href-shop").unwrap(),
            crawl_enabled: true,
            crawl_root: Some(CrawlRoot::from_presentation_url(ROOT).unwrap()),
        }])
        .await
        .unwrap();
    listing_source_id
}

async fn insert_frontier(pool: &PgPool, listing_source_id: ListingSourceId, urls: &[&str]) {
    sqlx::query(
        "INSERT INTO listing_source_crawl_frontier (listing_source_id, url) \
         SELECT $1, url FROM UNNEST($2::text[]) AS url",
    )
    .bind(listing_source_id.as_uuid())
    .bind(urls)
    .execute(pool)
    .await
    .unwrap();
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_drop_only_frontier_urls_that_repeat_their_first_path_segment() {
    let pool = get_postgres_client().await;
    let listing_source_id = insert_listing_source(&pool).await;
    let made_up = [
        "https://www.shop.example.com/en-GB/latest-items_pg_904/en-GB/glass/goblet/prod_13036",
        "https://www.shop.example.com/en-GB//en-GB//page_902",
        "https://www.shop.example.com/recent-acquisitions/recent-acquisitions/desk-refno-4144/",
        "https://www.shop.example.com/antikes/sitze/antikes/sitze/ueber-mich/",
        "https://www.shop.example.com/en-GB/about-us/en-GB?page=2",
    ];
    let kept = [
        "https://www.shop.example.com/en-GB/glass/goblet/prod_13036",
        "https://www.shop.example.com/en-GB/latest-items_pg_904/page_904?page=2",
        "https://www.shop.example.com/en-GB/en-GB-guide/page_1",
        "https://www.shop.example.com/",
    ];
    insert_frontier(&pool, listing_source_id, &made_up).await;
    insert_frontier(&pool, listing_source_id, &kept).await;

    sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();

    let mut frontier: Vec<String> = CrawlFrontierRepositoryImpl::new(pool.clone())
        .load_frontier(&listing_source_id)
        .await
        .unwrap()
        .into_iter()
        .map(String::from)
        .collect();
    frontier.sort_unstable();
    let mut expected: Vec<String> = kept.iter().map(|url| (*url).to_owned()).collect();
    expected.sort_unstable();
    assert_eq!(frontier, expected);
}
