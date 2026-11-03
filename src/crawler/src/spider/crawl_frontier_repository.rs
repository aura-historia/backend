//! The seeds a domain's next crawl starts from besides its root: the frontier its previous crawl
//! left unvisited, and the URLs an incremental crawl does not fetch again: its product URLs and
//! the URLs retired as duplicates of another
//! (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).

use async_trait::async_trait;
use sqlx::PgPool;
use std::collections::HashSet;
use url::Url;

use crate::CrawlerDomainId;
use crate::spider::classification::url_metadata::{CrawlerDisposition, UrlClass};
use crate::spider::crawled_root::{CrawlOutcomeWrite, CrawledRoot};
use crate::spider::utils::url::CrawledUrl;

/// Most known product URLs an incremental crawl skips; a larger shop's further product pages are
/// fetched again, which costs requests but loses nothing.
pub const MAX_KNOWN_URLS: i64 = 100_000;

/// The stored URLs an incremental crawl does not fetch again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownUrls {
    /// The domain's product URLs, which the crawl reports as known product pages it found linked.
    pub products: Vec<Url>,
    /// The URLs retired as duplicates of another, which are no product pages of their own.
    pub retired: Vec<Url>,
}

#[async_trait]
#[mockall::automock]
pub trait CrawlFrontierRepository: Send + Sync {
    /// The URLs the domain's previous crawl left unvisited, without any whose page the crawler
    /// retired as a duplicate since, under either trailing-slash spelling.
    async fn load_frontier(&self, domain_id: &CrawlerDomainId) -> Result<Vec<Url>, sqlx::Error>;

    /// The product URLs the domain already stores and the URLs it retired as duplicates, at most
    /// [`MAX_KNOWN_URLS`] together.
    async fn known_urls(&self, domain_id: &CrawlerDomainId) -> Result<KnownUrls, sqlx::Error>;

    /// Replaces the domain's frontier with what its latest crawl, of `root`, left unvisited,
    /// while the domain still has that crawl root.
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

/// Stored URLs were absolute when written; one that no longer parses is invalid persisted state.
fn persisted_urls(rows: Vec<String>) -> Result<Vec<Url>, sqlx::Error> {
    rows.into_iter()
        .map(|url| Url::parse(&url).map_err(|error| sqlx::Error::Decode(Box::new(error))))
        .collect()
}

#[async_trait]
impl CrawlFrontierRepository for CrawlFrontierRepositoryImpl {
    async fn load_frontier(&self, domain_id: &CrawlerDomainId) -> Result<Vec<Url>, sqlx::Error> {
        let frontier = persisted_urls(
            sqlx::query_scalar::<_, String>(
                "SELECT url FROM listing_source_domain_crawl_frontier
                 WHERE domain_id = $1
                 ORDER BY url",
            )
            .bind(domain_id.as_uuid())
            .fetch_all(&self.pool)
            .await?,
        )?;
        // Storage may keep a retired page under either trailing-slash spelling.
        let spellings_of = |url: &Url| CrawledUrl::new(url.clone()).page_spellings();
        let spellings: Vec<String> = frontier
            .iter()
            .flat_map(spellings_of)
            .map(String::from)
            .collect();
        let retired: HashSet<String> = sqlx::query_scalar::<_, String>(
            "SELECT url FROM listing_source_urls
             WHERE url = ANY($1::text[]) AND crawler_disposition = $2",
        )
        .bind(&spellings)
        .bind(CrawlerDisposition::Duplicate.as_str())
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect();
        Ok(frontier
            .into_iter()
            .filter(|url| {
                !spellings_of(url)
                    .iter()
                    .any(|spelling| retired.contains(spelling.as_str()))
            })
            .collect())
    }

    async fn known_urls(&self, domain_id: &CrawlerDomainId) -> Result<KnownUrls, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, bool)>(
            "SELECT url, crawler_disposition = $3 AS retired FROM listing_source_urls
             WHERE domain_id = $1 AND (url_class = $2 OR crawler_disposition = $3)
             ORDER BY url
             LIMIT $4",
        )
        .bind(domain_id.as_uuid())
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

    /// The share lock on the domain row holds back a move to another root, which updates that
    /// row and drops the frontier, until the new frontier is committed; a move that committed
    /// first leaves no row on the crawled root to lock.
    async fn replace_frontier(
        &self,
        root: &CrawledRoot,
        frontier: &[Url],
    ) -> Result<CrawlOutcomeWrite, sqlx::Error> {
        let urls: Vec<&str> = frontier.iter().map(Url::as_str).collect();
        let mut transaction = self.pool.begin().await?;
        let domain_has_root = sqlx::query_scalar::<_, i32>(
            "SELECT 1 FROM listing_source_domains
             WHERE domain_id = $1 AND crawl_root_host = $2
             FOR SHARE",
        )
        .bind(root.domain_id.as_uuid())
        .bind(&root.crawl_root_host)
        .fetch_optional(&mut *transaction)
        .await?
        .is_some();
        if !domain_has_root {
            return Ok(CrawlOutcomeWrite::RootMoved);
        }
        sqlx::query("DELETE FROM listing_source_domain_crawl_frontier WHERE domain_id = $1")
            .bind(root.domain_id.as_uuid())
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO listing_source_domain_crawl_frontier (domain_id, url)
             SELECT $1, url FROM UNNEST($2::text[]) AS frontier(url)
             ON CONFLICT DO NOTHING",
        )
        .bind(root.domain_id.as_uuid())
        .bind(urls)
        .execute(&mut *transaction)
        .await?;
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
}
