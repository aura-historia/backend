//! `GET /api/listing_sources`: the operator's ListingSource overview. The contract is in
//! `docs/crawler/operations.md` (review service).

use crate::review::model::{CrawlStateOverview, ListingSourceOverview, UrlErrorKindCount};
use crate::review::repository::{crawler_review_id_from_uuid, listing_source_id_from_uuid};
use crate::scraper::removal_guard::MASS_REMOVAL_HELD_ERROR_KIND;
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

    /// Lists ListingSources, those that need an operator first: a crawl-enabled ListingSource
    /// whose crawl waits for an operator, with URLs blocked on a pending review, a scrape failure
    /// streak or held removals is never cut off by `limit`, which only bounds the others.
    pub async fn list_listing_sources(
        &self,
        limit: i64,
    ) -> Result<Vec<ListingSourceOverview>, sqlx::Error> {
        let rows = sqlx::query(
            "WITH blocking AS (
                -- The oldest pending review a ListingSource's blocked URLs wait on: a review a
                -- URL recorded as its wait, or the first product-schema review of a ListingSource
                -- without a product schema.
                SELECT DISTINCT ON (listing_source_id) listing_source_id, review_id
                FROM (
                    SELECT su.listing_source_id, cr.review_id, cr.created
                    FROM listing_source_urls su
                    JOIN crawler_reviews cr ON cr.review_id = su.schema_review_waited_on
                                           AND cr.status = 'PENDING_REVIEW'
                                           AND su.last_error_kind = 'PendingSchemaReview'
                    WHERE su.url_class = 'product'
                      AND su.crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
                    UNION ALL
                    SELECT pr.listing_source_id, pr.review_id, pr.created
                    FROM crawler_reviews pr
                    WHERE pr.artifact_type = 'PRODUCT_SCHEMA'
                      AND pr.status = 'PENDING_REVIEW'
                      AND NOT EXISTS (
                          SELECT 1
                          FROM listing_source_product_schemas ps
                          WHERE ps.listing_source_id = pr.listing_source_id
                            AND (jsonb_typeof(ps.product_schema) <> 'array'
                                 OR jsonb_array_length(ps.product_schema) > 0)
                      )
                ) waited_on
                ORDER BY listing_source_id, created, review_id
            ), overview AS (
                SELECT
                    s.listing_source_id,
                    s.listing_source_name,
                    s.crawl_enabled,
                    s.llm_calls_count,
                    s.updated,
                    s.crawl_root_url,
                    s.last_crawled AS last_successful_crawl,
                    s.last_crawl_outcome,
                    s.last_crawl_error_kind,
                    s.crawl_failure_count,
                    s.next_crawl_at,
                    s.scrape_failure_streak,
                    s.last_scrape_error_kind,
                    s.scrape_failing_since,
                    s.next_scrape_at AS scrape_paused_until,
                    u.last_successful_scrape,
                    COALESCE(r.pending_reviews, 0) AS pending_reviews,
                    COALESCE(u.product_urls, 0) AS product_urls,
                    COALESCE(b.blocked_urls, 0) AS blocked_urls,
                    b.blocked_since,
                    CASE WHEN b.blocked_urls IS NOT NULL THEN bl.review_id END AS blocking_review_id,
                    COALESCE(u.held_removal_urls, 0) AS held_removal_urls,
                    u.held_since,
                    COALESCE(u.missing_value_urls, 0) AS missing_value_urls,
                    s.last_crawl_outcome IS NOT DISTINCT FROM $2 AS needs_operator
                FROM listing_sources s
                LEFT JOIN (
                    SELECT listing_source_id, COUNT(*) AS pending_reviews
                    FROM crawler_reviews
                    WHERE status = 'PENDING_REVIEW'
                    GROUP BY listing_source_id
                ) r ON r.listing_source_id = s.listing_source_id
                LEFT JOIN (
                    SELECT listing_source_id,
                           COUNT(*) FILTER (WHERE url_class = 'product') AS product_urls,
                           COUNT(*) FILTER (WHERE url_class = 'product'
                                              AND last_error_kind = $3) AS held_removal_urls,
                           COUNT(*) FILTER (WHERE url_class = 'product'
                                              AND last_error_kind = 'MissingRequiredValue') AS missing_value_urls,
                           MAX(last_scraped) FILTER (WHERE url_class = 'product'
                                                       AND last_error_kind IS DISTINCT FROM $3) AS last_successful_scrape,
                           MIN(last_scraped) FILTER (WHERE url_class = 'product'
                                                       AND last_error_kind = $3) AS held_since
                    FROM listing_source_urls
                    WHERE crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
                    GROUP BY listing_source_id
                ) u ON u.listing_source_id = s.listing_source_id
                LEFT JOIN (
                    SELECT su.listing_source_id, COUNT(*) AS blocked_urls,
                           LEAST(MIN(cr.created), MIN(fs.created)) AS blocked_since
                    FROM listing_source_urls su
                    LEFT JOIN crawler_reviews cr ON cr.review_id = su.schema_review_waited_on
                                                AND cr.status = 'PENDING_REVIEW'
                                                AND su.last_error_kind = 'PendingSchemaReview'
                    LEFT JOIN (
                        -- A ListingSource without a product schema scrapes none of its URLs while
                        -- its first product-schema review is pending.
                        SELECT pr.listing_source_id, MIN(pr.created) AS created
                        FROM crawler_reviews pr
                        WHERE pr.artifact_type = 'PRODUCT_SCHEMA'
                          AND pr.status = 'PENDING_REVIEW'
                          AND NOT EXISTS (
                              SELECT 1
                              FROM listing_source_product_schemas ps
                              WHERE ps.listing_source_id = pr.listing_source_id
                                AND (jsonb_typeof(ps.product_schema) <> 'array'
                                     OR jsonb_array_length(ps.product_schema) > 0)
                          )
                        GROUP BY pr.listing_source_id
                    ) fs ON fs.listing_source_id = su.listing_source_id
                    WHERE su.url_class = 'product'
                      AND su.crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
                      AND (cr.review_id IS NOT NULL OR fs.listing_source_id IS NOT NULL)
                    GROUP BY su.listing_source_id
                ) b ON b.listing_source_id = s.listing_source_id
                LEFT JOIN blocking bl ON bl.listing_source_id = s.listing_source_id
            ), ranked AS (
                SELECT overview.*,
                       crawl_enabled AND (needs_operator OR blocked_urls > 0
                                          OR scrape_failure_streak > 0
                                          OR held_removal_urls > 0) AS needs_attention,
                       ROW_NUMBER() OVER (
                           ORDER BY crawl_enabled DESC,
                                    needs_operator DESC,
                                    (blocked_urls > 0) DESC,
                                    scrape_failure_streak DESC,
                                    (held_removal_urls > 0) DESC,
                                    pending_reviews DESC, updated DESC
                       ) AS position
                FROM overview
             )
             SELECT * FROM ranked
             WHERE needs_attention OR position <= $1
             ORDER BY position",
        )
            .bind(limit)
            .bind(NEEDS_OPERATOR_CRAWL_OUTCOME)
            .bind(MASS_REMOVAL_HELD_ERROR_KIND)
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
                crawl_root_url: row.try_get("crawl_root_url")?,
                last_successful_crawl: row.try_get("last_successful_crawl")?,
                last_successful_scrape: row.try_get("last_successful_scrape")?,
                pending_reviews: row.try_get("pending_reviews")?,
                product_urls: row.try_get("product_urls")?,
                blocked_urls: row.try_get("blocked_urls")?,
                blocked_since: row.try_get("blocked_since")?,
                blocking_review_id: row
                    .try_get::<Option<uuid::Uuid>, _>("blocking_review_id")?
                    .map(crawler_review_id_from_uuid)
                    .transpose()?,
                held_removal_urls: row.try_get("held_removal_urls")?,
                held_since: row.try_get("held_since")?,
                url_error_kinds: Vec::new(),
                missing_value_urls: row.try_get("missing_value_urls")?,
                crawl_state: CrawlStateOverview {
                    last_crawl_outcome: row.try_get("last_crawl_outcome")?,
                    last_crawl_error_kind: row.try_get("last_crawl_error_kind")?,
                    crawl_failure_count: row.try_get("crawl_failure_count")?,
                    next_crawl_at: row.try_get("next_crawl_at")?,
                    scrape_failure_streak: row.try_get("scrape_failure_streak")?,
                    last_scrape_error_kind: row.try_get("last_scrape_error_kind")?,
                    scrape_failing_since: row.try_get("scrape_failing_since")?,
                    scrape_paused_until: row.try_get("scrape_paused_until")?,
                },
            });
        }
        let listing_source_ids = listing_sources
            .iter()
            .map(|overview| uuid::Uuid::from(overview.listing_source_id))
            .collect::<Vec<_>>();
        let mut error_kinds = self.list_url_error_kinds(&listing_source_ids).await?;
        for overview in &mut listing_sources {
            overview.url_error_kinds = error_kinds
                .remove(&uuid::Uuid::from(overview.listing_source_id))
                .unwrap_or_default();
        }
        Ok(listing_sources)
    }

    /// The last error kinds of the product URLs per ListingSource with their URL counts, the most
    /// frequent first.
    async fn list_url_error_kinds(
        &self,
        listing_source_ids: &[uuid::Uuid],
    ) -> Result<HashMap<uuid::Uuid, Vec<UrlErrorKindCount>>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT listing_source_id, last_error_kind, COUNT(*) AS url_count
             FROM listing_source_urls
             WHERE listing_source_id = ANY($1)
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_error_kind IS NOT NULL
             GROUP BY listing_source_id, last_error_kind
             ORDER BY url_count DESC, last_error_kind",
        )
        .bind(listing_source_ids)
        .fetch_all(&self.pool)
        .await?;
        let mut by_listing_source: HashMap<uuid::Uuid, Vec<UrlErrorKindCount>> = HashMap::new();
        for row in rows {
            by_listing_source
                .entry(row.try_get("listing_source_id")?)
                .or_default()
                .push(UrlErrorKindCount {
                    kind: row.try_get("last_error_kind")?,
                    count: row.try_get("url_count")?,
                });
        }
        Ok(by_listing_source)
    }
}
