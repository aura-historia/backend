//! `GET /api/listing_sources`: the operator's ListingSource overview. The contract is in
//! `docs/crawler/operations.md` (review service).

use crate::review::model::{DomainCrawlOverview, ListingSourceOverview};
use crate::review::repository::{crawler_domain_id_from_uuid, listing_source_id_from_uuid};
use crate::spider::candidate_service::NEEDS_OPERATOR_CRAWL_OUTCOME;
use sqlx::{PgPool, Row};
use std::collections::HashMap;

/// Reads the ListingSource overview of the console.
#[derive(Clone)]
pub struct ListingSourceOverviewReader {
    pool: PgPool,
}

impl ListingSourceOverviewReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Enabled ListingSources with a domain waiting for an operator come first: nothing else
    /// in the console points at them, so the limit must not cut them off.
    pub async fn list_listing_sources(
        &self,
        limit: i64,
    ) -> Result<Vec<ListingSourceOverview>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT
                s.listing_source_id,
                s.listing_source_name,
                s.crawl_enabled,
                s.llm_calls_count,
                COALESCE(d.domain_count, 0) AS domain_count,
                d.last_successful_crawl,
                u.last_successful_scrape,
                COALESCE(r.pending_reviews, 0) AS pending_reviews,
                COALESCE(u.product_urls, 0) AS product_urls,
                COALESCE(u.blocked_urls, 0) AS blocked_urls,
                COALESCE(u.missing_value_urls, 0) AS missing_value_urls
             FROM listing_sources s
             LEFT JOIN (
                SELECT listing_source_id, COUNT(*) AS domain_count, MAX(last_crawled) AS last_successful_crawl,
                       COUNT(*) FILTER (WHERE last_crawl_outcome = $2) AS needs_operator_domains
                FROM listing_source_domains GROUP BY listing_source_id
             ) d ON d.listing_source_id = s.listing_source_id
             LEFT JOIN (
                SELECT listing_source_id, COUNT(*) AS pending_reviews
                FROM crawler_reviews
                WHERE status = 'PENDING_REVIEW'
                GROUP BY listing_source_id
             ) r ON r.listing_source_id = s.listing_source_id
             LEFT JOIN (
                SELECT listing_source_id,
                       COUNT(*) FILTER (WHERE url_class = 'product') AS product_urls,
                       COUNT(*) FILTER (WHERE last_error_kind IN ('PendingSchemaReview', 'PendingUrlPatternReview')) AS blocked_urls,
                       COUNT(*) FILTER (WHERE url_class = 'product'
                                          AND last_error_kind = 'MissingRequiredValue') AS missing_value_urls,
                       MAX(last_scraped) FILTER (WHERE url_class = 'product') AS last_successful_scrape
                FROM listing_source_urls
                WHERE crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
                GROUP BY listing_source_id
             ) u ON u.listing_source_id = s.listing_source_id
             ORDER BY s.crawl_enabled DESC, COALESCE(d.needs_operator_domains, 0) > 0 DESC,
                      pending_reviews DESC, s.updated DESC
             LIMIT $1",
        )
            .bind(limit)
            .bind(NEEDS_OPERATOR_CRAWL_OUTCOME)
            .fetch_all(&self.pool)
            .await?;

        let mut listing_sources = Vec::with_capacity(rows.len());
        for row in rows {
            listing_sources.push(ListingSourceOverview {
                listing_source_id: listing_source_id_from_uuid(
                    row.try_get::<uuid::Uuid, _>("listing_source_id")?,
                )?,
                listing_source_name: row.try_get("listing_source_name")?,
                crawl_enabled: row.try_get("crawl_enabled")?,
                llm_calls_count: row.try_get("llm_calls_count")?,
                domain_count: row.try_get("domain_count")?,
                last_successful_crawl: row.try_get("last_successful_crawl")?,
                last_successful_scrape: row.try_get("last_successful_scrape")?,
                pending_reviews: row.try_get("pending_reviews")?,
                product_urls: row.try_get("product_urls")?,
                blocked_urls: row.try_get("blocked_urls")?,
                missing_value_urls: row.try_get("missing_value_urls")?,
                domains: Vec::new(),
            });
        }
        let mut domains = self
            .list_domain_states(
                &listing_sources
                    .iter()
                    .map(|overview| uuid::Uuid::from(overview.listing_source_id))
                    .collect::<Vec<_>>(),
            )
            .await?;
        for overview in &mut listing_sources {
            overview.domains = domains
                .remove(&uuid::Uuid::from(overview.listing_source_id))
                .unwrap_or_default();
        }
        Ok(listing_sources)
    }

    /// The spider's and the scraper's last recorded state per domain, grouped by ListingSource.
    async fn list_domain_states(
        &self,
        listing_source_ids: &[uuid::Uuid],
    ) -> Result<HashMap<uuid::Uuid, Vec<DomainCrawlOverview>>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT listing_source_id, domain_id, listing_source_domain, last_crawled,
                    last_crawl_outcome, last_crawl_error_kind, crawl_failure_count, next_crawl_at,
                    scrape_failure_streak, last_scrape_error_kind
             FROM listing_source_domains
             WHERE listing_source_id = ANY($1)
             ORDER BY listing_source_domain",
        )
        .bind(listing_source_ids)
        .fetch_all(&self.pool)
        .await?;
        let mut by_listing_source: HashMap<uuid::Uuid, Vec<DomainCrawlOverview>> = HashMap::new();
        for row in rows {
            by_listing_source
                .entry(row.try_get("listing_source_id")?)
                .or_default()
                .push(DomainCrawlOverview {
                    domain_id: crawler_domain_id_from_uuid(row.try_get("domain_id")?)?,
                    domain: row.try_get("listing_source_domain")?,
                    last_crawled: row.try_get("last_crawled")?,
                    last_crawl_outcome: row.try_get("last_crawl_outcome")?,
                    last_crawl_error_kind: row.try_get("last_crawl_error_kind")?,
                    crawl_failure_count: row.try_get("crawl_failure_count")?,
                    next_crawl_at: row.try_get("next_crawl_at")?,
                    scrape_failure_streak: row.try_get("scrape_failure_streak")?,
                    last_scrape_error_kind: row.try_get("last_scrape_error_kind")?,
                });
        }
        Ok(by_listing_source)
    }
}
