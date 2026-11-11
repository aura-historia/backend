use crate::network::policy::{PublicTargetError, url_on_crawl_host, validate_public_http_url};
use crate::review::repository::RejectedPages;
use crate::spider::classification::url_metadata::{
    CrawlerDisposition, CrawlerUrlWriteOutcome, UrlClass,
};
use crate::spider::utils::url::{CrawledUrl, legacy_spelling_renames, stored_spellings};
use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use sqlx::{FromRow, PgPool, Row};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct SpiderUrlRecord {
    pub listing_source_id: ListingSourceId,
    pub url: url::Url,
    pub url_class: UrlClass,
    pub disposition: CrawlerDisposition,
    pub last_scraped_hash: Option<String>,
    pub last_scraped: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for SpiderUrlRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let listing_source_id: uuid::Uuid = row.try_get("listing_source_id")?;
        let url = url::Url::parse(&row.try_get::<String, _>("url")?)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        let url_class = row
            .try_get::<String, _>("url_class")?
            .parse()
            .map_err(|error: String| sqlx::Error::Decode(error.into()))?;
        let disposition = row
            .try_get::<String, _>("crawler_disposition")?
            .parse()
            .map_err(|error: String| sqlx::Error::Decode(error.into()))?;
        Ok(Self {
            listing_source_id: ListingSourceId::try_from(listing_source_id)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            url,
            url_class,
            disposition,
            last_scraped_hash: row.try_get("last_scraped_hash")?,
            last_scraped: row.try_get("last_scraped")?,
            created: row.try_get("created")?,
            updated: row.try_get("updated")?,
        })
    }
}

/// A URL the spider found, with the class it is stored under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredUrl {
    pub url: url::Url,
    pub url_class: UrlClass,
    /// The canonical URL of this product page when the page is a duplicate of it
    /// (`docs/crawler/networking.md#canonical-product-urls`): the page is stored there instead.
    pub canonical: Option<url::Url>,
}

#[derive(Debug, thiserror::Error)]
pub enum UrlMetadataRepositoryError {
    #[error("ListingSource has no crawl root")]
    ListingSourceNotCrawled { listing_source_id: ListingSourceId },
    #[error("URL is not a supported crawler HTTP URL")]
    InvalidCrawlerUrl {
        url: url::Url,
        reason: PublicTargetError,
    },
    #[error("URL host does not match the ListingSource's crawl host")]
    UrlHostDoesNotMatchCrawlHost { url: url::Url, crawl_host: String },
    #[error("URL is already owned by another ListingSource")]
    UrlOwnedByAnotherListingSource {
        url: url::Url,
        requested_listing_source_id: ListingSourceId,
        current_listing_source_id: ListingSourceId,
    },
    #[error("crawler URL persistence failed")]
    Database {
        #[source]
        source: sqlx::Error,
    },
}

#[async_trait]
#[mockall::automock]
pub trait UrlMetadataRepository: Send + Sync {
    async fn upsert_link(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        url_class: &UrlClass,
    ) -> Result<SpiderUrlRecord, UrlMetadataRepositoryError>;

    async fn upsert_links_batch(
        &self,
        listing_source_id: &ListingSourceId,
        links: &[DiscoveredUrl],
    ) -> Result<Vec<SpiderUrlRecord>, UrlMetadataRepositoryError>;

    async fn set_disposition(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        disposition: CrawlerDisposition,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;

    /// Up to `limit` of the URLs the ListingSource stores, in a stable order by a hash of the
    /// URL, so a sample drawn from them reaches every part of what earlier crawls found.
    async fn stored_sample_urls(
        &self,
        listing_source_id: &ListingSourceId,
        limit: usize,
    ) -> Result<Vec<String>, sqlx::Error>;
}

pub struct UrlMetadataRepositoryImpl {
    pool: PgPool,
}

impl UrlMetadataRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The crawl host of the ListingSource the URLs are written for, share-locked for the
    /// transaction.
    async fn locked_crawl_host(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
    ) -> Result<String, UrlMetadataRepositoryError> {
        let crawl_host = sqlx::query_scalar::<_, Option<String>>(
            "SELECT crawl_host FROM listing_sources \
             WHERE listing_source_id = $1 \
             FOR KEY SHARE",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?
        .flatten();
        crawl_host.ok_or(UrlMetadataRepositoryError::ListingSourceNotCrawled { listing_source_id })
    }

    async fn validate_existing_url_ownership(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        urls: &[String],
    ) -> Result<(), UrlMetadataRepositoryError> {
        let rows = sqlx::query(
            "SELECT url, listing_source_id FROM listing_source_urls \
             WHERE url = ANY($1::text[]) ORDER BY url FOR UPDATE",
        )
        .bind(urls)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;

        for row in rows {
            let raw_url = row.try_get::<String, _>("url").map_err(database_error)?;
            let url = url::Url::parse(&raw_url).map_err(|error| {
                UrlMetadataRepositoryError::Database {
                    source: sqlx::Error::Decode(Box::new(error)),
                }
            })?;
            let current_listing_source_id = ListingSourceId::try_from(
                row.try_get::<uuid::Uuid, _>("listing_source_id")
                    .map_err(database_error)?,
            )
            .map_err(|error| UrlMetadataRepositoryError::Database {
                source: sqlx::Error::Decode(Box::new(error)),
            })?;
            if current_listing_source_id != listing_source_id {
                return Err(UrlMetadataRepositoryError::UrlOwnedByAnotherListingSource {
                    url,
                    requested_listing_source_id: listing_source_id,
                    current_listing_source_id,
                });
            }
        }
        Ok(())
    }

    /// The spelling each of `urls` is stored under, with its class: a page the shop links both
    /// with and without the trailing slash keeps one row, under the slashed spelling, whether the
    /// two spellings arrive in one batch or the slashed one already has a row of this
    /// ListingSource ([`stored_spellings`]).
    async fn stored_spellings(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        urls: &[url::Url],
        url_classes: &[UrlClass],
    ) -> Result<Vec<(url::Url, UrlClass)>, UrlMetadataRepositoryError> {
        let slashed_twins = urls
            .iter()
            .filter_map(|url| CrawledUrl::new(url.clone()).slashed_twin())
            .map(String::from)
            .collect::<Vec<_>>();
        let stored_twins = if slashed_twins.is_empty() {
            HashSet::new()
        } else {
            sqlx::query_scalar::<_, String>(
                "SELECT url FROM listing_source_urls \
                 WHERE url = ANY($2::text[]) AND listing_source_id = $1 \
                 ORDER BY url FOR UPDATE",
            )
            .bind(uuid::Uuid::from(listing_source_id))
            .bind(&slashed_twins)
            .fetch_all(&mut **transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .collect::<HashSet<_>>()
        };
        let fetched = urls
            .iter()
            .cloned()
            .zip(url_classes.iter().copied())
            .collect::<Vec<_>>();
        Ok(stored_spellings(&fetched, |slashed| {
            stored_twins.contains(slashed.as_str())
        }))
    }

    /// Moves rows stored under an earlier spelling of a rediscovered URL to the fetched spelling,
    /// so the page keeps its row, scrape state and raw-capture identity instead of gaining a
    /// second row. Only rows of this ListingSource move, and never onto a URL that already
    /// has a row.
    async fn adopt_legacy_spellings(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        urls: &[url::Url],
    ) -> Result<(), UrlMetadataRepositoryError> {
        let mut renamed = 0;
        for round in legacy_spelling_renames(urls) {
            if round.is_empty() {
                continue;
            }
            let (legacy, fetched): (Vec<String>, Vec<String>) = round
                .iter()
                .map(|rename| (rename.legacy.to_string(), rename.fetched.to_string()))
                .unzip();
            renamed += sqlx::query(
                "UPDATE listing_source_urls AS stored SET url = input.fetched \
                 FROM UNNEST($2::text[], $3::text[]) AS input(legacy, fetched) \
                 WHERE stored.url = input.legacy \
                   AND stored.listing_source_id = $1 \
                   AND NOT EXISTS (SELECT 1 FROM listing_source_urls existing WHERE existing.url = input.fetched)",
            )
            .bind(uuid::Uuid::from(listing_source_id))
            .bind(&legacy)
            .bind(&fetched)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?
            .rows_affected();
        }
        if renamed > 0 {
            tracing::info!(
                renamed,
                "Crawler URLs moved from their earlier spelling to the fetched spelling"
            );
        }
        Ok(())
    }

    /// `links` with the canonical URLs storage may use. A canonical URL is dropped, and the page
    /// stored as fetched, when it names the page itself in another spelling, when it is no crawler
    /// URL of this source's crawl host, when a rejection covers the page or its canonical URL (the
    /// rejection keeps the fetched page `other`), when another ListingSource owns it, when it is
    /// itself retired as a duplicate, when its row stays a non-product row through this write
    /// (the write reclassifies only `ACTIVE` rows), or when the batch canonicalizes a page onto a
    /// page that is itself canonicalized, so no row is moved along a chain the shop may not mean.
    /// Pages are compared whatever their trailing slash.
    async fn admit_canonical_urls(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        crawl_host: &str,
        rejected: &RejectedPages,
        links: &[DiscoveredUrl],
    ) -> Result<Vec<DiscoveredUrl>, UrlMetadataRepositoryError> {
        let page_of = |url: &url::Url| CrawledUrl::new(url.clone()).page_spelling();
        let mut links = links.to_vec();
        for link in &mut links {
            link.canonical = link.canonical.take().filter(|canonical| {
                page_of(canonical) != page_of(&link.url)
                    && canonical.fragment().is_none()
                    && validate_public_http_url(canonical).is_ok()
                    && url_on_crawl_host(canonical, crawl_host)
                    && !rejected.covers(link.url.as_str())
                    && !rejected.covers(canonical.as_str())
            });
        }
        let sources = links
            .iter()
            .filter(|link| link.canonical.is_some())
            .map(|link| page_of(&link.url))
            .collect::<HashSet<_>>();
        let targets = links
            .iter()
            .filter_map(|link| link.canonical.as_ref().map(page_of))
            .collect::<HashSet<_>>();
        let target_spellings = links
            .iter()
            .filter_map(|link| link.canonical.clone())
            .flat_map(|target| CrawledUrl::new(target).page_spellings())
            .map(String::from)
            .collect::<Vec<_>>();
        let unusable_targets = if target_spellings.is_empty() {
            HashSet::new()
        } else {
            sqlx::query_scalar::<_, String>(
                "SELECT url FROM listing_source_urls \
                 WHERE url = ANY($2::text[]) \
                   AND (listing_source_id <> $1 \
                        OR crawler_disposition = $3 \
                        OR (url_class <> $4 AND crawler_disposition <> $5))",
            )
            .bind(uuid::Uuid::from(listing_source_id))
            .bind(&target_spellings)
            .bind(CrawlerDisposition::Duplicate.as_str())
            .bind(UrlClass::ProductListing.as_str())
            .bind(CrawlerDisposition::Active.as_str())
            .fetch_all(&mut **transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .filter_map(|url| url::Url::parse(&url).ok())
            .map(|url| page_of(&url))
            .collect::<HashSet<_>>()
        };
        for link in &mut links {
            link.canonical = link.canonical.take().filter(|canonical| {
                let target = page_of(canonical);
                !targets.contains(&page_of(&link.url))
                    && !sources.contains(&target)
                    && !unusable_targets.contains(&target)
            });
        }
        Ok(links)
    }

    /// Collapses the ListingSource's product rows stored under a spelling the crawler no longer
    /// produces onto the spelling it does (`docs/crawler/networking.md#stored-urls`): a row whose
    /// URL differs from its own [`CrawledUrl`] spelling, such as one stored before a query
    /// parameter counted as presentation, is a duplicate of that spelling. It moves there while
    /// no row has the spelling and is retired as `DUPLICATE` otherwise, as
    /// [`Self::collapse_duplicate_rows`] does for a canonicalized page. One query reads the
    /// source's rows, so the spider calls it once per pass of the source, not per URL. The read
    /// runs outside a transaction: only a source with such a row opens one, which locks and
    /// re-reads the rows it changes, so a pass that has nothing to collapse holds no lock and
    /// returns its connection after the read.
    pub async fn collapse_unnormalized_product_urls(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<(), UrlMetadataRepositoryError> {
        let stored = sqlx::query_scalar::<_, String>(
            "SELECT url FROM listing_source_urls \
             WHERE listing_source_id = $1 AND url_class = $2 \
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD') \
             ORDER BY url COLLATE \"C\"",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(UrlClass::ProductListing.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        let unnormalized = unnormalized_rows(stored)?;
        if unnormalized.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        Self::locked_crawl_host(&mut transaction, *listing_source_id).await?;
        Self::collapse_duplicate_rows(&mut transaction, *listing_source_id, &unnormalized).await?;
        transaction.commit().await.map_err(database_error)
    }

    /// Moves or retires the rows a canonicalized page was stored under before
    /// (`docs/crawler/networking.md#canonical-product-urls`). While no spelling of the canonical
    /// URL has a row, the first such row moves there and keeps its scrape state and raw-capture
    /// identity; every other row of the duplicate is recorded `DUPLICATE`, so neither the
    /// scraper nor a later crawl selects it again. A row already retired stays as it is.
    async fn collapse_duplicate_rows(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        duplicates: &[DuplicateRows],
    ) -> Result<(), UrlMetadataRepositoryError> {
        if duplicates.is_empty() {
            return Ok(());
        }
        let duplicate_spellings = duplicates
            .iter()
            .flat_map(|duplicate| duplicate.spellings.iter().map(ToString::to_string))
            .collect::<Vec<_>>();
        let stored_duplicates = sqlx::query_scalar::<_, String>(
            "SELECT url FROM listing_source_urls \
             WHERE url = ANY($2::text[]) AND listing_source_id = $1 \
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD') \
             ORDER BY url FOR UPDATE",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(&duplicate_spellings)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .collect::<HashSet<_>>();
        if stored_duplicates.is_empty() {
            return Ok(());
        }
        let target_spellings = duplicates
            .iter()
            .flat_map(|duplicate| CrawledUrl::new(duplicate.canonical.clone()).page_spellings())
            .map(String::from)
            .collect::<Vec<_>>();
        let mut stored_targets = sqlx::query_scalar::<_, String>(
            "SELECT url FROM listing_source_urls WHERE url = ANY($1::text[]) ORDER BY url FOR UPDATE",
        )
        .bind(&target_spellings)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .filter_map(|url| url::Url::parse(&url).ok())
        .map(|url| CrawledUrl::new(url).page_spelling())
        .collect::<HashSet<_>>();

        let mut moves = Vec::<(String, String)>::new();
        let mut retired = Vec::<String>::new();
        let mut handled = HashSet::<String>::new();
        for duplicate in duplicates {
            let target_page = CrawledUrl::new(duplicate.canonical.clone()).page_spelling();
            for spelling in &duplicate.spellings {
                let spelling = spelling.to_string();
                if !stored_duplicates.contains(&spelling) || !handled.insert(spelling.clone()) {
                    continue;
                }
                if stored_targets.insert(target_page.clone()) {
                    moves.push((spelling, duplicate.canonical.to_string()));
                } else {
                    retired.push(spelling);
                }
            }
        }

        let (sources, targets): (Vec<String>, Vec<String>) = moves.into_iter().unzip();
        let moved = sqlx::query(
            "UPDATE listing_source_urls AS stored SET url = input.target, updated = NOW() \
             FROM UNNEST($2::text[], $3::text[]) AS input(source, target) \
             WHERE stored.url = input.source \
               AND stored.listing_source_id = $1",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(&sources)
        .bind(&targets)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        let retired = sqlx::query(
            "UPDATE listing_source_urls \
             SET crawler_disposition = $3, next_retry_at = NULL, updated = NOW() \
             WHERE url = ANY($2::text[]) \
               AND listing_source_id = $1 \
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(&retired)
        .bind(CrawlerDisposition::Duplicate.as_str())
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        if moved + retired > 0 {
            tracing::info!(
                event = "crawler.spider.duplicate_urls_collapsed",
                moved,
                retired,
                "Crawler URLs of duplicate product pages moved to or retired in favour of their canonical URL"
            );
        }
        Ok(())
    }

    fn verify_url_hosts(
        urls: &[url::Url],
        crawl_host: &str,
    ) -> Result<(), UrlMetadataRepositoryError> {
        for url in urls {
            if url.fragment().is_some() {
                return Err(UrlMetadataRepositoryError::InvalidCrawlerUrl {
                    url: url.clone(),
                    reason: PublicTargetError::InvalidUrl,
                });
            }
            if let Err(reason) = validate_public_http_url(url) {
                return Err(UrlMetadataRepositoryError::InvalidCrawlerUrl {
                    url: url.clone(),
                    reason,
                });
            }
        }
        urls.iter()
            .find(|url| !url_on_crawl_host(url, crawl_host))
            .map_or(Ok(()), |url| {
                Err(UrlMetadataRepositoryError::UrlHostDoesNotMatchCrawlHost {
                    url: url.clone(),
                    crawl_host: crawl_host.to_string(),
                })
            })
    }
}

#[async_trait]
impl UrlMetadataRepository for UrlMetadataRepositoryImpl {
    async fn stored_sample_urls(
        &self,
        listing_source_id: &ListingSourceId,
        limit: usize,
    ) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar::<_, String>(
            "SELECT url FROM listing_source_urls WHERE listing_source_id = $1 \
             ORDER BY md5(url), url LIMIT $2",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
    }

    async fn upsert_link(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        url_class: &UrlClass,
    ) -> Result<SpiderUrlRecord, UrlMetadataRepositoryError> {
        self.upsert_links_batch(
            listing_source_id,
            &[DiscoveredUrl {
                url: url.clone(),
                url_class: *url_class,
                canonical: None,
            }],
        )
        .await?
        .pop()
        .ok_or(UrlMetadataRepositoryError::Database {
            source: sqlx::Error::RowNotFound,
        })
    }

    async fn upsert_links_batch(
        &self,
        listing_source_id: &ListingSourceId,
        links: &[DiscoveredUrl],
    ) -> Result<Vec<SpiderUrlRecord>, UrlMetadataRepositoryError> {
        if links.is_empty() {
            return Ok(Vec::new());
        }
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let crawl_host = Self::locked_crawl_host(&mut transaction, *listing_source_id).await?;
        let fetched = links
            .iter()
            .map(|link| link.url.clone())
            .collect::<Vec<_>>();
        Self::verify_url_hosts(&fetched, &crawl_host)?;
        Self::validate_existing_url_ownership(
            &mut transaction,
            *listing_source_id,
            &fetched.iter().map(ToString::to_string).collect::<Vec<_>>(),
        )
        .await?;
        // An operator's rejection outranks the URL pattern: the page stays `other` instead of
        // being scraped again only to be classified `other` by the rejection check.
        let rejected = RejectedPages::load(&mut *transaction, listing_source_id)
            .await
            .map_err(database_error)?;
        let links = Self::admit_canonical_urls(
            &mut transaction,
            *listing_source_id,
            &crawl_host,
            &rejected,
            links,
        )
        .await?;
        let (urls, url_classes): (Vec<url::Url>, Vec<UrlClass>) =
            canonical_batch(&links).into_iter().unzip();
        let (urls, url_classes): (Vec<url::Url>, Vec<UrlClass>) =
            Self::stored_spellings(&mut transaction, *listing_source_id, &urls, &url_classes)
                .await?
                .into_iter()
                .unzip();
        let url_classes: Vec<UrlClass> = urls
            .iter()
            .zip(url_classes)
            .map(|(url, class)| {
                if rejected.covers(url.as_str()) {
                    UrlClass::Other
                } else {
                    class
                }
            })
            .collect();
        Self::adopt_legacy_spellings(&mut transaction, *listing_source_id, &urls).await?;
        Self::collapse_duplicate_rows(
            &mut transaction,
            *listing_source_id,
            &duplicate_rows_of(&links, &urls),
        )
        .await?;
        let url_strings = urls.iter().map(ToString::to_string).collect::<Vec<_>>();

        let records = sqlx::query_as::<_, SpiderUrlRecord>(
            "INSERT INTO listing_source_urls (listing_source_id, url, raw_source_record_key, url_class, created, updated) \
             SELECT $1, input.url, input.url, input.url_class, NOW(), NOW() \
             FROM UNNEST($2::text[], $3::text[]) AS input(url, url_class) \
             ON CONFLICT (url) DO UPDATE SET \
                 url_class = CASE \
                     WHEN listing_source_urls.crawler_disposition = 'ACTIVE' THEN EXCLUDED.url_class \
                     ELSE listing_source_urls.url_class \
                 END, \
                 updated = CASE \
                     WHEN listing_source_urls.crawler_disposition = 'ACTIVE' THEN NOW() \
                     ELSE listing_source_urls.updated \
                 END \
             WHERE listing_source_urls.listing_source_id = EXCLUDED.listing_source_id \
             RETURNING listing_source_id, url, url_class, crawler_disposition, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(&url_strings)
        .bind(url_classes.iter().map(ToString::to_string).collect::<Vec<_>>())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;

        if records.len() != urls.len() {
            Self::validate_existing_url_ownership(
                &mut transaction,
                *listing_source_id,
                &url_strings,
            )
            .await?;
            return Err(UrlMetadataRepositoryError::Database {
                source: sqlx::Error::Protocol(
                    "crawler URL batch returned fewer rows than inputs".to_owned(),
                ),
            });
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(records)
    }

    async fn set_disposition(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        disposition: CrawlerDisposition,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_source_urls \
             SET crawler_disposition = $3, updated = NOW() \
             WHERE listing_source_id = $1 \
               AND url = $2 \
               AND crawler_disposition = 'ACTIVE'",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(url.as_str())
        .bind(disposition.as_str())
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }
}

fn database_error(source: sqlx::Error) -> UrlMetadataRepositoryError {
    UrlMetadataRepositoryError::Database { source }
}

/// The URLs and classes `links` are stored under: each page under its canonical URL when it has
/// one. A canonical URL the batch already stores, fetched itself or as the canonical URL of an
/// earlier page, is stored once; the batch's other URLs stay as given, so a batch that names one
/// fetched URL twice is still refused.
fn canonical_batch(links: &[DiscoveredUrl]) -> Vec<(url::Url, UrlClass)> {
    let mut stored = links
        .iter()
        .filter(|link| link.canonical.is_none())
        .map(|link| link.url.clone())
        .collect::<HashSet<_>>();
    links
        .iter()
        .filter_map(|link| match &link.canonical {
            None => Some((link.url.clone(), link.url_class)),
            Some(canonical) => stored
                .insert(canonical.clone())
                .then(|| (canonical.clone(), link.url_class)),
        })
        .collect()
}

/// The rows a canonicalized page may have been stored under, and the spelling its canonical URL
/// is stored under in this batch.
#[derive(Debug)]
struct DuplicateRows {
    spellings: Vec<url::Url>,
    canonical: url::Url,
}

/// The stored URLs whose [`CrawledUrl`] spelling differs from themselves, each as a duplicate of
/// that spelling. A stored URL that no longer parses is invalid persisted state.
fn unnormalized_rows(
    stored: Vec<String>,
) -> Result<Vec<DuplicateRows>, UrlMetadataRepositoryError> {
    let mut unnormalized = Vec::new();
    for stored in stored {
        let url = url::Url::parse(&stored)
            .map_err(|error| database_error(sqlx::Error::Decode(Box::new(error))))?;
        let normalized = CrawledUrl::new(url.clone()).as_url().clone();
        if normalized != url {
            unnormalized.push(DuplicateRows {
                spellings: vec![url],
                canonical: normalized,
            });
        }
    }
    Ok(unnormalized)
}

/// The [`DuplicateRows`] of each canonicalized page of `links`: the page's own spellings and
/// earlier spellings, with its canonical URL in the spelling among `stored` that names its page.
/// A spelling that names the canonical URL's page, or that the batch stores as a page of its own,
/// is no row of the duplicate: an earlier spelling may be exactly the canonical URL (`+` for
/// `%20`).
fn duplicate_rows_of(links: &[DiscoveredUrl], stored: &[url::Url]) -> Vec<DuplicateRows> {
    let stored_by_page = stored
        .iter()
        .map(|url| (CrawledUrl::new(url.clone()).page_spelling(), url))
        .collect::<std::collections::HashMap<_, _>>();
    let stored = stored.iter().collect::<HashSet<_>>();
    let mut duplicates = links
        .iter()
        .filter_map(|link| {
            let canonical_page = CrawledUrl::new(link.canonical.clone()?).page_spelling();
            let canonical = stored_by_page
                .get(&canonical_page)
                .map_or_else(|| link.canonical.clone(), |url| Some((*url).clone()))?;
            let page = CrawledUrl::new(link.url.clone());
            let mut spellings = Vec::new();
            for spelling in page
                .page_spellings()
                .into_iter()
                .chain(page.legacy_spellings())
            {
                if !spellings.contains(&spelling)
                    && !stored.contains(&spelling)
                    && CrawledUrl::new(spelling.clone()).page_spelling() != canonical_page
                {
                    spellings.push(spelling);
                }
            }
            Some(DuplicateRows {
                spellings,
                canonical,
            })
        })
        .filter(|duplicate| !duplicate.spellings.is_empty())
        .collect::<Vec<_>>();
    duplicates.sort_by(|left, right| left.spellings[0].as_str().cmp(right.spellings[0].as_str()));
    duplicates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_collapse_only_stored_urls_the_crawler_now_spells_differently() {
        let unnormalized = unnormalized_rows(vec![
            "https://shop.example/item/clean-1".to_owned(),
            "https://shop.example/item/twin-2?cur=EUR".to_owned(),
        ])
        .unwrap();

        let [row] = unnormalized.as_slice() else {
            panic!("one stored URL needs collapsing: {unnormalized:?}");
        };
        assert_eq!(
            row.spellings,
            [url::Url::parse("https://shop.example/item/twin-2?cur=EUR").unwrap()]
        );
        assert_eq!(row.canonical.as_str(), "https://shop.example/item/twin-2");
    }

    #[test]
    fn should_fail_closed_when_a_stored_url_no_longer_parses() {
        let error = unnormalized_rows(vec!["/relative/1".to_owned()]).unwrap_err();

        assert!(
            matches!(
                error,
                UrlMetadataRepositoryError::Database {
                    source: sqlx::Error::Decode(_)
                }
            ),
            "{error:?}"
        );
    }
}
