use crate::CrawlerDomainId;
use crate::network::policy::{
    PublicTargetError, url_matches_configured_domain, validate_public_http_url,
};
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
    pub domain_id: CrawlerDomainId,
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
        let domain_id: uuid::Uuid = row.try_get("domain_id")?;
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
            domain_id: CrawlerDomainId::try_from(domain_id)
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
    #[error("crawler domain does not belong to ListingSource")]
    DomainNotOwnedByListingSource {
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
    },
    #[error("URL is not a supported crawler HTTP URL")]
    InvalidCrawlerUrl {
        url: url::Url,
        reason: PublicTargetError,
    },
    #[error("URL host does not match the configured crawler domain")]
    UrlHostDoesNotMatchDomain { url: url::Url, domain: String },
    #[error("URL is already owned by another ListingSource")]
    UrlOwnedByAnotherListingSource {
        url: url::Url,
        requested_listing_source_id: ListingSourceId,
        current_listing_source_id: ListingSourceId,
    },
    #[error("URL is already owned by another crawler domain")]
    UrlOwnedByAnotherDomain {
        url: url::Url,
        current_domain_id: CrawlerDomainId,
        requested_domain_id: CrawlerDomainId,
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
        domain_id: &CrawlerDomainId,
        url: &url::Url,
        url_class: &UrlClass,
    ) -> Result<SpiderUrlRecord, UrlMetadataRepositoryError>;

    async fn upsert_links_batch(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        links: &[DiscoveredUrl],
    ) -> Result<Vec<SpiderUrlRecord>, UrlMetadataRepositoryError>;

    async fn set_disposition(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        disposition: CrawlerDisposition,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
}

pub struct UrlMetadataRepositoryImpl {
    pool: PgPool,
}

impl UrlMetadataRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn verify_domain_owner(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
    ) -> Result<String, UrlMetadataRepositoryError> {
        let domain = sqlx::query_scalar::<_, String>(
            "SELECT listing_source_domain FROM listing_source_domains \
             WHERE listing_source_id = $1 AND domain_id = $2 \
             FOR KEY SHARE",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(domain_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?;
        domain.ok_or(UrlMetadataRepositoryError::DomainNotOwnedByListingSource {
            listing_source_id,
            domain_id,
        })
    }

    async fn validate_existing_url_ownership(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
        urls: &[String],
    ) -> Result<(), UrlMetadataRepositoryError> {
        let rows = sqlx::query(
            "SELECT url, listing_source_id, domain_id FROM listing_source_urls \
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
            let current_domain_id = CrawlerDomainId::try_from(
                row.try_get::<uuid::Uuid, _>("domain_id")
                    .map_err(database_error)?,
            )
            .map_err(|error| UrlMetadataRepositoryError::Database {
                source: sqlx::Error::Decode(Box::new(error)),
            })?;
            if current_domain_id != domain_id {
                return Err(UrlMetadataRepositoryError::UrlOwnedByAnotherDomain {
                    url,
                    current_domain_id,
                    requested_domain_id: domain_id,
                });
            }
        }
        Ok(())
    }

    /// The spelling each of `urls` is stored under, with its class: a page the shop links both
    /// with and without the trailing slash keeps one row, under the slashed spelling, whether the
    /// two spellings arrive in one batch or the slashed one already has a row of this
    /// ListingSource domain ([`stored_spellings`]).
    async fn stored_spellings(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
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
                 WHERE url = ANY($3::text[]) AND listing_source_id = $1 AND domain_id = $2 \
                 ORDER BY url FOR UPDATE",
            )
            .bind(uuid::Uuid::from(listing_source_id))
            .bind(domain_id.as_uuid())
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
    /// second row. Only rows of this ListingSource domain move, and never onto a URL that already
    /// has a row.
    async fn adopt_legacy_spellings(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
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
                 FROM UNNEST($3::text[], $4::text[]) AS input(legacy, fetched) \
                 WHERE stored.url = input.legacy \
                   AND stored.listing_source_id = $1 \
                   AND stored.domain_id = $2 \
                   AND NOT EXISTS (SELECT 1 FROM listing_source_urls existing WHERE existing.url = input.fetched)",
            )
            .bind(uuid::Uuid::from(listing_source_id))
            .bind(domain_id.as_uuid())
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
    /// URL of this domain, when a rejection covers the page or its canonical URL (the rejection
    /// keeps the fetched page `other`), when another ListingSource or domain owns it, when it is
    /// itself retired as a duplicate, or when the batch canonicalizes a page onto a page that is
    /// itself canonicalized, so no row is moved along a chain the shop may not mean. Pages are
    /// compared whatever their trailing slash.
    async fn admit_canonical_urls(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
        domain: &str,
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
                    && url_matches_configured_domain(canonical, domain)
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
                 WHERE url = ANY($3::text[]) \
                   AND (listing_source_id <> $1 OR domain_id <> $2 \
                        OR crawler_disposition = $4)",
            )
            .bind(uuid::Uuid::from(listing_source_id))
            .bind(domain_id.as_uuid())
            .bind(&target_spellings)
            .bind(CrawlerDisposition::Duplicate.as_str())
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

    /// Moves or retires the rows a canonicalized page was stored under before
    /// (`docs/crawler/networking.md#canonical-product-urls`). While no spelling of the canonical
    /// URL has a row, the first such row moves there and keeps its scrape state and raw-capture
    /// identity; every other row of the duplicate is recorded `DUPLICATE`, so neither the
    /// scraper nor a later crawl selects it again. A row already retired stays as it is.
    async fn collapse_duplicate_rows(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
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
             WHERE url = ANY($3::text[]) AND listing_source_id = $1 AND domain_id = $2 \
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD') \
             ORDER BY url FOR UPDATE",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(domain_id.as_uuid())
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
        for duplicate in duplicates {
            let target_page = CrawledUrl::new(duplicate.canonical.clone()).page_spelling();
            for spelling in &duplicate.spellings {
                let spelling = spelling.to_string();
                if !stored_duplicates.contains(&spelling)
                    || moves.iter().any(|(source, _)| *source == spelling)
                    || retired.contains(&spelling)
                {
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
             FROM UNNEST($3::text[], $4::text[]) AS input(source, target) \
             WHERE stored.url = input.source \
               AND stored.listing_source_id = $1 \
               AND stored.domain_id = $2",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(domain_id.as_uuid())
        .bind(&sources)
        .bind(&targets)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        let retired = sqlx::query(
            "UPDATE listing_source_urls \
             SET crawler_disposition = $4, next_retry_at = NULL, updated = NOW() \
             WHERE url = ANY($3::text[]) \
               AND listing_source_id = $1 \
               AND domain_id = $2 \
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(domain_id.as_uuid())
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

    fn verify_url_hosts(urls: &[url::Url], domain: &str) -> Result<(), UrlMetadataRepositoryError> {
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
            .find(|url| !url_matches_configured_domain(url, domain))
            .map_or(Ok(()), |url| {
                Err(UrlMetadataRepositoryError::UrlHostDoesNotMatchDomain {
                    url: url.clone(),
                    domain: domain.to_string(),
                })
            })
    }
}

#[async_trait]
impl UrlMetadataRepository for UrlMetadataRepositoryImpl {
    async fn upsert_link(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        url: &url::Url,
        url_class: &UrlClass,
    ) -> Result<SpiderUrlRecord, UrlMetadataRepositoryError> {
        self.upsert_links_batch(
            listing_source_id,
            domain_id,
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
        domain_id: &CrawlerDomainId,
        links: &[DiscoveredUrl],
    ) -> Result<Vec<SpiderUrlRecord>, UrlMetadataRepositoryError> {
        if links.is_empty() {
            return Ok(Vec::new());
        }
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let domain =
            Self::verify_domain_owner(&mut transaction, *listing_source_id, *domain_id).await?;
        let fetched = links
            .iter()
            .map(|link| link.url.clone())
            .collect::<Vec<_>>();
        Self::verify_url_hosts(&fetched, &domain)?;
        Self::validate_existing_url_ownership(
            &mut transaction,
            *listing_source_id,
            *domain_id,
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
            *domain_id,
            &domain,
            &rejected,
            links,
        )
        .await?;
        let (urls, url_classes): (Vec<url::Url>, Vec<UrlClass>) =
            canonical_batch(&links).into_iter().unzip();
        let (urls, url_classes): (Vec<url::Url>, Vec<UrlClass>) = Self::stored_spellings(
            &mut transaction,
            *listing_source_id,
            *domain_id,
            &urls,
            &url_classes,
        )
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
        Self::adopt_legacy_spellings(&mut transaction, *listing_source_id, *domain_id, &urls)
            .await?;
        Self::collapse_duplicate_rows(
            &mut transaction,
            *listing_source_id,
            *domain_id,
            &duplicate_rows_of(&links, &urls),
        )
        .await?;
        let url_strings = urls.iter().map(ToString::to_string).collect::<Vec<_>>();

        let records = sqlx::query_as::<_, SpiderUrlRecord>(
            "INSERT INTO listing_source_urls (listing_source_id, domain_id, url, raw_source_record_key, url_class, created, updated) \
             SELECT $1, $2, input.url, input.url, input.url_class, NOW(), NOW() \
             FROM UNNEST($3::text[], $4::text[]) AS input(url, url_class) \
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
               AND listing_source_urls.domain_id = EXCLUDED.domain_id \
             RETURNING listing_source_id, domain_id, url, url_class, crawler_disposition, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id.as_uuid())
        .bind(&url_strings)
        .bind(url_classes.iter().map(ToString::to_string).collect::<Vec<_>>())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;

        if records.len() != urls.len() {
            Self::validate_existing_url_ownership(
                &mut transaction,
                *listing_source_id,
                *domain_id,
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
