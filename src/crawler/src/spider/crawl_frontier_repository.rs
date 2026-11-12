//! The seeds a ListingSource's next crawl starts from besides its root: the frontier its previous crawl
//! left unvisited, and the URLs an incremental crawl does not fetch again: its product URLs and
//! the URLs retired as duplicates of another
//! (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).

use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use sqlx::PgPool;
use std::collections::{BTreeSet, HashSet};
use url::Url;

use crate::spider::classification::url_metadata::{CrawlerDisposition, UrlClass};
use crate::spider::classification::url_metadata_repository::{
    UrlMetadataRepositoryError, UrlMetadataRepositoryImpl,
};
use crate::spider::crawled_root::{CrawlOutcomeWrite, CrawledRoot};
use crate::spider::utils::url::CrawledUrl;

/// Most known product URLs an incremental crawl skips; a larger shop's further product pages are
/// fetched again, which costs requests but loses nothing.
pub const MAX_KNOWN_URLS: i64 = 100_000;

/// Most frontier URLs one statement looks up, deletes or inserts, so a large frontier is read and
/// written in short statements instead of one that holds its connection for seconds.
const FRONTIER_CHUNK: usize = 1_000;

/// The stored URLs an incremental crawl does not fetch again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownUrls {
    /// The source's product URLs, which the crawl reports as known product pages it found linked.
    pub products: Vec<Url>,
    /// The URLs retired as duplicates of another, which are no product pages of their own.
    pub retired: Vec<Url>,
}

#[async_trait]
#[mockall::automock]
pub trait CrawlFrontierRepository: Send + Sync {
    /// Collapses the source's stored product URLs that the crawler would now spell differently
    /// onto that spelling, so a crawl pass starts from rows the scheduler does not scrape twice.
    async fn collapse_unnormalized_product_urls(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<(), UrlMetadataRepositoryError>;

    /// The URLs the source's previous crawl left unvisited, without any whose page the crawler
    /// retired as a duplicate since, under either trailing-slash spelling.
    async fn load_frontier(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Vec<Url>, sqlx::Error>;

    /// The product URLs the source already stores and the URLs it retired as duplicates, at most
    /// [`MAX_KNOWN_URLS`] together.
    async fn known_urls(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<KnownUrls, sqlx::Error>;

    /// Replaces the source's frontier with what its latest crawl, of `root`, left unvisited,
    /// while the source still has that crawl root.
    async fn replace_frontier(
        &self,
        root: &CrawledRoot,
        frontier: &[Url],
    ) -> Result<CrawlOutcomeWrite, sqlx::Error>;
}

pub struct CrawlFrontierRepositoryImpl {
    pool: PgPool,
}

impl CrawlFrontierRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// What turns a stored frontier into the one a crawl left: the URLs to delete and to insert, each
/// sorted. URLs in both stay as they are.
#[derive(Debug, Default, PartialEq, Eq)]
struct FrontierChange<'a> {
    removed: Vec<&'a str>,
    added: Vec<&'a str>,
}

impl<'a> FrontierChange<'a> {
    fn between(stored: &'a BTreeSet<String>, left: &'a BTreeSet<&'a str>) -> Self {
        Self {
            removed: stored
                .iter()
                .map(String::as_str)
                .filter(|url| !left.contains(url))
                .collect(),
            added: left
                .iter()
                .copied()
                .filter(|url| !stored.contains(*url))
                .collect(),
        }
    }
}

/// Stored URLs were absolute when written; one that no longer parses is invalid persisted state.
fn persisted_urls(rows: Vec<String>) -> Result<Vec<Url>, sqlx::Error> {
    rows.into_iter()
        .map(|url| Url::parse(&url).map_err(|error| sqlx::Error::Decode(Box::new(error))))
        .collect()
}

#[async_trait]
impl CrawlFrontierRepository for CrawlFrontierRepositoryImpl {
    async fn collapse_unnormalized_product_urls(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<(), UrlMetadataRepositoryError> {
        UrlMetadataRepositoryImpl::new(self.pool.clone())
            .collapse_unnormalized_product_urls(listing_source_id)
            .await
    }

    async fn load_frontier(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Vec<Url>, sqlx::Error> {
        let frontier = persisted_urls(
            sqlx::query_scalar::<_, String>(
                "SELECT url FROM listing_source_crawl_frontier
                 WHERE listing_source_id = $1
                 ORDER BY url",
            )
            .bind(listing_source_id.as_uuid())
            .fetch_all(&self.pool)
            .await?,
        )?;
        // Storage may keep a retired page under either trailing-slash spelling.
        let spellings_of = |url: &Url| CrawledUrl::new(url.clone()).page_spellings();
        let mut retired = HashSet::new();
        for chunk in frontier.chunks(FRONTIER_CHUNK) {
            let spellings: Vec<String> = chunk
                .iter()
                .flat_map(spellings_of)
                .map(String::from)
                .collect();
            retired.extend(
                sqlx::query_scalar::<_, String>(
                    "SELECT url FROM listing_source_urls
                     WHERE url = ANY($1::text[]) AND crawler_disposition = $2",
                )
                .bind(&spellings)
                .bind(CrawlerDisposition::Duplicate.as_str())
                .fetch_all(&self.pool)
                .await?,
            );
        }
        Ok(frontier
            .into_iter()
            .filter(|url| {
                !spellings_of(url)
                    .iter()
                    .any(|spelling| retired.contains(spelling.as_str()))
            })
            .collect())
    }

    async fn known_urls(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<KnownUrls, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, bool)>(
            "SELECT url, crawler_disposition = $3 AS retired FROM listing_source_urls
             WHERE listing_source_id = $1 AND (url_class = $2 OR crawler_disposition = $3)
             ORDER BY url
             LIMIT $4",
        )
        .bind(listing_source_id.as_uuid())
        .bind(UrlClass::ProductListing.as_str())
        .bind(CrawlerDisposition::Duplicate.as_str())
        .bind(MAX_KNOWN_URLS)
        .fetch_all(&self.pool)
        .await?;
        let (retired, products): (Vec<_>, Vec<_>) =
            rows.into_iter().partition(|(_, retired)| *retired);
        let urls = |rows: Vec<(String, bool)>| {
            persisted_urls(rows.into_iter().map(|(url, _)| url).collect())
        };
        Ok(KnownUrls {
            products: urls(products)?,
            retired: urls(retired)?,
        })
    }

    /// The share lock on the source row holds back a move to another root, which updates that
    /// row and drops the frontier, until the new frontier is committed; a move that committed
    /// first leaves no row on the crawled root to lock. Only the URLs that changed are written,
    /// at most [`FRONTIER_CHUNK`] per statement.
    async fn replace_frontier(
        &self,
        root: &CrawledRoot,
        frontier: &[Url],
    ) -> Result<CrawlOutcomeWrite, sqlx::Error> {
        let left: BTreeSet<&str> = frontier.iter().map(Url::as_str).collect();
        let mut transaction = self.pool.begin().await?;
        let source_has_root = sqlx::query_scalar::<_, i32>(
            "SELECT 1 FROM listing_sources
             WHERE listing_source_id = $1 AND crawl_root_url = $2
             FOR SHARE",
        )
        .bind(root.listing_source_id.as_uuid())
        .bind(&root.crawl_root_url)
        .fetch_optional(&mut *transaction)
        .await?
        .is_some();
        if !source_has_root {
            return Ok(CrawlOutcomeWrite::RootMoved);
        }
        let stored: BTreeSet<String> = sqlx::query_scalar(
            "SELECT url FROM listing_source_crawl_frontier WHERE listing_source_id = $1",
        )
        .bind(root.listing_source_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .collect();
        let change = FrontierChange::between(&stored, &left);
        for removed in change.removed.chunks(FRONTIER_CHUNK) {
            sqlx::query(
                "DELETE FROM listing_source_crawl_frontier
                 WHERE listing_source_id = $1 AND url = ANY($2::text[])",
            )
            .bind(root.listing_source_id.as_uuid())
            .bind(removed)
            .execute(&mut *transaction)
            .await?;
        }
        for added in change.added.chunks(FRONTIER_CHUNK) {
            sqlx::query(
                "INSERT INTO listing_source_crawl_frontier (listing_source_id, url)
                 SELECT $1, url FROM UNNEST($2::text[]) AS frontier(url)
                 ON CONFLICT DO NOTHING",
            )
            .bind(root.listing_source_id.as_uuid())
            .bind(added)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(CrawlOutcomeWrite::Recorded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_fail_closed_on_a_persisted_url_that_no_longer_parses() {
        let parsed = persisted_urls(vec!["https://shop.example/deep/1".to_string()]).unwrap();
        assert_eq!(parsed, [Url::parse("https://shop.example/deep/1").unwrap()]);

        let error = persisted_urls(vec![
            "https://shop.example/deep/1".to_string(),
            "/relative/2".to_string(),
        ])
        .unwrap_err();
        assert!(matches!(error, sqlx::Error::Decode(_)), "{error:?}");
    }

    #[test]
    fn should_write_only_changed_urls_when_a_crawl_replaces_the_frontier() {
        let stored: BTreeSet<String> = ["https://shop.example/a", "https://shop.example/b"]
            .map(String::from)
            .into();
        let left: BTreeSet<&str> = ["https://shop.example/c", "https://shop.example/b"].into();

        assert_eq!(
            FrontierChange::between(&stored, &left),
            FrontierChange {
                removed: vec!["https://shop.example/a"],
                added: vec!["https://shop.example/c"],
            }
        );
    }

    #[test]
    fn should_write_nothing_when_the_frontier_is_unchanged() {
        let stored: BTreeSet<String> = ["https://shop.example/a".to_string()].into();
        let left: BTreeSet<&str> = ["https://shop.example/a"].into();

        assert_eq!(
            FrontierChange::between(&stored, &left),
            FrontierChange::default()
        );
    }
}
