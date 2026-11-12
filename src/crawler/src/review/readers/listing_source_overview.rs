//! `GET /api/listing_sources`: the operator's ListingSource overview, one window at a time, and the
//! health counts of the first page. The contract is in `docs/crawler/operations.md` (review
//! service). Every filter, the search, the order and the count run in SQL over all
//! ListingSources.

use crate::review::model::{
    CrawlStateOverview, ListPage, ListWindow, ListingSourceOverview, ReviewActor, SearchTerm,
    SortDirection, SourceActivity, SourceSort, SourceStatus, UrlErrorKindCount,
};
use crate::review::repository::{crawler_review_id_from_uuid, listing_source_id_from_uuid};
use crate::scraper::removal_guard::MASS_REMOVAL_HELD_ERROR_KIND;
use crate::spider::candidate_service::NEEDS_OPERATOR_CRAWL_OUTCOME;
use crate::spider::local_lock::{LocalLockManager, RunningWork, WorkLockScope};
use listing_source_core::ListingSourceId;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::Arc;
use time::OffsetDateTime;

/// Which window of the ListingSource overview to read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListingSourceQuery {
    /// ListingSources whose name or crawl host contains this text.
    pub search: Option<SearchTerm>,
    pub activity: Option<SourceActivity>,
    pub status: Option<SourceStatus>,
    pub sort: SourceSort,
    pub direction: SortDirection,
    pub window: ListWindow,
}

/// How many ListingSources are in each condition that needs an operator. The conditions overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthCounts {
    pub needs_attention: i64,
    pub crawl_failing: i64,
    pub crawl_needs_operator: i64,
    pub blocked_by_review: i64,
    pub failing_to_scrape: i64,
    pub held_removals: i64,
}

/// The ListingSources with everything the overview shows, computed in SQL for every ListingSource
/// the query may list. Binds `$1`, the one ListingSource to read or null for all, `$2`, the crawl
/// outcome that waits for an operator, and `$3`, the error kind of a held removal.
const OVERVIEW_CTES: &str = "WITH blocking AS (
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
        s.crawl_host,
        s.crawl_enabled,
        s.updated,
        s.crawl_root_url,
        s.last_crawled AS last_successful_crawl,
        s.last_crawl_outcome,
        s.last_crawl_outcome IS NOT DISTINCT FROM 'FAILED' AS crawl_failed,
        s.last_crawl_error_kind,
        s.crawl_failure_count,
        s.next_crawl_at,
        s.scrape_failure_streak,
        s.last_scrape_status_code,
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
        CASE WHEN s.mass_removal_confirmed_until > NOW() THEN s.mass_removal_confirmed_until END
            AS mass_removal_confirmed_until,
        CASE WHEN s.mass_removal_confirmed_until > NOW() THEN s.mass_removal_confirmed_by END
            AS mass_removal_confirmed_by,
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
    WHERE $1::uuid IS NULL OR s.listing_source_id = $1
";

/// Reads the ListingSource overview of the console.
#[derive(Clone)]
pub struct ListingSourceOverviewReader {
    pool: PgPool,
    work_locks: Arc<LocalLockManager>,
}

impl ListingSourceOverviewReader {
    /// A reader that reports no running work, as a console without a crawler in its process has.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            work_locks: Arc::new(LocalLockManager::new()),
        }
    }

    /// This reader reporting the crawler's running work from `work_locks`, the lock manager the
    /// spider and scraper of this process hold their work locks in.
    pub fn with_work_locks(self, work_locks: Arc<LocalLockManager>) -> Self {
        Self { work_locks, ..self }
    }

    pub(super) fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// How many ListingSources this process holds a `scope` work lock on right now.
    pub(super) fn running_count(&self, scope: WorkLockScope) -> i64 {
        let running = self
            .work_locks
            .running_work()
            .iter()
            .filter(|work| work.scope == scope)
            .count();
        i64::try_from(running).unwrap_or(i64::MAX)
    }

    /// The window of ListingSources `query` asks for, with how many match it in all. The rows
    /// leave out the URL error kinds, which [`Self::find_listing_source`] adds.
    pub async fn list_listing_sources(
        &self,
        query: &ListingSourceQuery,
    ) -> Result<ListPage<ListingSourceOverview>, sqlx::Error> {
        let (rows, total) = self.read_overviews(query, None).await?;
        Ok(ListPage::new(rows, total, query.window))
    }

    /// The overview of `listing_source_id` with its URL error kinds, or `None` when no such
    /// ListingSource exists.
    pub async fn find_listing_source(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<Option<ListingSourceOverview>, sqlx::Error> {
        let (mut rows, _) = self
            .read_overviews(&ListingSourceQuery::default(), Some(listing_source_id))
            .await?;
        let Some(mut overview) = rows.pop() else {
            return Ok(None);
        };
        overview.url_error_kinds = self
            .list_url_error_kinds(&[*listing_source_id.as_uuid()])
            .await?
            .remove(listing_source_id.as_uuid())
            .unwrap_or_default();
        Ok(Some(overview))
    }

    /// How many ListingSources are in each condition that needs an operator.
    pub async fn health_counts(&self) -> Result<HealthCounts, sqlx::Error> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "{OVERVIEW_CTES}
             ), ranked AS ({RANKED_SELECT})
             SELECT COUNT(*) FILTER (WHERE needs_attention) AS needs_attention,
                    COUNT(*) FILTER (WHERE crawl_enabled AND crawl_failed) AS crawl_failing,
                    COUNT(*) FILTER (WHERE crawl_enabled AND needs_operator) AS crawl_needs_operator,
                    COUNT(*) FILTER (WHERE crawl_enabled AND blocked_urls > 0) AS blocked_by_review,
                    COUNT(*) FILTER (WHERE crawl_enabled AND scrape_failure_streak > 0)
                        AS failing_to_scrape,
                    COUNT(*) FILTER (WHERE crawl_enabled AND held_removal_urls > 0) AS held_removals
             FROM ranked"
        )))
        .bind(None::<uuid::Uuid>)
        .bind(NEEDS_OPERATOR_CRAWL_OUTCOME)
        .bind(MASS_REMOVAL_HELD_ERROR_KIND)
        .fetch_one(&self.pool)
        .await?;
        Ok(HealthCounts {
            needs_attention: row.try_get("needs_attention")?,
            crawl_failing: row.try_get("crawl_failing")?,
            crawl_needs_operator: row.try_get("crawl_needs_operator")?,
            blocked_by_review: row.try_get("blocked_by_review")?,
            failing_to_scrape: row.try_get("failing_to_scrape")?,
            held_removals: row.try_get("held_removals")?,
        })
    }

    /// The overviews `query` asks for and how many ListingSources match it in all. `only`
    /// restricts the read to that ListingSource.
    async fn read_overviews(
        &self,
        query: &ListingSourceQuery,
        only: Option<ListingSourceId>,
    ) -> Result<(Vec<ListingSourceOverview>, i64), sqlx::Error> {
        let running = self.work_locks.running_work();
        let running_ids = |scope| {
            running
                .iter()
                .filter(|work| work.scope == scope)
                .map(|work| uuid::Uuid::from(work.listing_source_id))
                .collect::<Vec<_>>()
        };
        let now = OffsetDateTime::now_utc();
        let rows = sqlx::query(sqlx::AssertSqlSafe(overview_page_sql(
            query.sort,
            query.direction,
        )))
        .bind(only.map(|id| *id.as_uuid()))
        .bind(NEEDS_OPERATOR_CRAWL_OUTCOME)
        .bind(MASS_REMOVAL_HELD_ERROR_KIND)
        .bind(running_ids(WorkLockScope::Spider))
        .bind(running_ids(WorkLockScope::Scraper))
        .bind(query.search.as_ref().map(SearchTerm::contains_pattern))
        .bind(query.activity.map(activity_name))
        .bind(query.status.map(status_name))
        .bind(i64::try_from(query.window.offset()).unwrap_or(i64::MAX))
        .bind(i64::try_from(query.window.limit()).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;

        let Some(total) = rows.first().map(|row| row.try_get("total")).transpose()? else {
            return Err(sqlx::Error::RowNotFound);
        };
        let mut listing_sources = Vec::with_capacity(rows.len());
        for row in &rows {
            if row
                .try_get::<Option<uuid::Uuid>, _>("listing_source_id")?
                .is_some()
            {
                listing_sources.push(overview_of_row(row, &running, now)?);
            }
        }
        Ok((listing_sources, total))
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

fn activity_name(activity: SourceActivity) -> &'static str {
    match activity {
        SourceActivity::Crawling => "crawling",
        SourceActivity::Scraping => "scraping",
        SourceActivity::Idle => "idle",
    }
}

fn status_name(status: SourceStatus) -> &'static str {
    match status {
        SourceStatus::NeedsAttention => "needs_attention",
        SourceStatus::Healthy => "healthy",
        SourceStatus::Disabled => "disabled",
        SourceStatus::CrawlFailing => "crawl_failing",
        SourceStatus::CrawlNeedsOperator => "crawl_needs_operator",
        SourceStatus::Blocked => "blocked",
        SourceStatus::Failing => "failing",
        SourceStatus::Held => "held",
    }
}

/// The overview rows with whether each needs attention: a crawl-enabled ListingSource whose last
/// crawl failed or waits for an operator, with blocked URLs, a scrape failure streak or held
/// removals.
const RANKED_SELECT: &str = "SELECT overview.*,
                    crawl_enabled AND (crawl_failed OR needs_operator OR blocked_urls > 0
                                       OR scrape_failure_streak > 0
                                       OR held_removal_urls > 0) AS needs_attention
             FROM overview";

/// The ListingSources of `OVERVIEW_CTES` filtered, ranked by `sort` in `direction`, and one window
/// of them. Binds `$1` to `$3` of [`OVERVIEW_CTES`], `$4` and `$5`, the ListingSources the spider
/// and the scraper of this process work right now, `$6`, the `ILIKE` pattern of the name or host
/// or null, `$7`, the activity filter or null, `$8`, the status filter or null, `$9`, the rows to
/// skip, and `$10`, the most rows to answer. Every row carries the total; an empty window is one
/// row with the total and no ListingSource.
///
/// The SQL is assembled from static fragments only, chosen by `sort` and `direction`, never from
/// request text.
fn overview_page_sql(sort: SourceSort, direction: SortDirection) -> String {
    format!(
        "{OVERVIEW_CTES}
         ), ranked AS ({RANKED_SELECT}),
         matching AS (
             SELECT * FROM ranked
             WHERE ($6::text IS NULL
                    OR listing_source_name ILIKE $6
                    OR crawl_host ILIKE $6)
               AND ($7::text IS NULL
                    OR ($7 = 'crawling' AND listing_source_id = ANY($4))
                    OR ($7 = 'scraping' AND listing_source_id = ANY($5))
                    OR ($7 = 'idle' AND NOT listing_source_id = ANY($4)
                                    AND NOT listing_source_id = ANY($5)))
               AND ($8::text IS NULL
                    OR ($8 = 'needs_attention' AND needs_attention)
                    OR ($8 = 'healthy' AND crawl_enabled AND NOT needs_attention)
                    OR ($8 = 'disabled' AND NOT crawl_enabled)
                    OR ($8 = 'crawl_failing' AND crawl_enabled AND crawl_failed)
                    OR ($8 = 'crawl_needs_operator' AND crawl_enabled AND needs_operator)
                    OR ($8 = 'blocked' AND crawl_enabled AND blocked_urls > 0)
                    OR ($8 = 'failing' AND crawl_enabled AND scrape_failure_streak > 0)
                    OR ($8 = 'held' AND crawl_enabled AND held_removal_urls > 0))
         ), ordered AS (
             SELECT matching.*, ROW_NUMBER() OVER (ORDER BY {order}) AS position FROM matching
         )
         SELECT t.total, p.*
         FROM (SELECT COUNT(*) AS total FROM matching) t
         LEFT JOIN (SELECT * FROM ordered WHERE position > $9 AND position <= $9 + $10) p ON TRUE
         ORDER BY p.position",
        order = order_by(sort, direction),
    )
}

/// The `ORDER BY` list of `sort` in `direction`: every key in that direction, nulls last, the
/// ListingSource id last. `Attention` ranks crawl-enabled sources first, then those whose crawl
/// waits for an operator, those whose last crawl failed, those with blocked URLs, the longest scrape failure streaks, those with
/// held removals, the most pending reviews and the most recently updated.
fn order_by(sort: SourceSort, direction: SortDirection) -> String {
    let keys: &[&str] = match sort {
        SourceSort::Attention => &[
            "crawl_enabled",
            "needs_operator",
            "crawl_failed",
            "(blocked_urls > 0)",
            "scrape_failure_streak",
            "(held_removal_urls > 0)",
            "pending_reviews",
            "updated",
        ],
        SourceSort::Name => &["lower(listing_source_name)"],
        SourceSort::PendingReviews => &["pending_reviews"],
        SourceSort::LastSuccessfulScrape => &["last_successful_scrape"],
        SourceSort::NextCrawlAt => &["next_crawl_at"],
    };
    let direction = direction.sql();
    keys.iter()
        .map(|key| format!("{key} {direction} NULLS LAST"))
        .chain(["listing_source_id".to_owned()])
        .collect::<Vec<_>>()
        .join(", ")
}

/// The overview `row` of the page query holds, with the work `running` at `now`.
fn overview_of_row(
    row: &PgRow,
    running: &[RunningWork],
    now: OffsetDateTime,
) -> Result<ListingSourceOverview, sqlx::Error> {
    let listing_source_id =
        listing_source_id_from_uuid(row.try_get::<uuid::Uuid, _>("listing_source_id")?)?;
    Ok(ListingSourceOverview {
        listing_source_id,
        listing_source_name: row.try_get("listing_source_name")?,
        crawl_enabled: row.try_get("crawl_enabled")?,
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
        mass_removal_confirmed_until: row.try_get("mass_removal_confirmed_until")?,
        mass_removal_confirmed_by: confirming_actor(row.try_get("mass_removal_confirmed_by")?)?,
        url_error_kinds: Vec::new(),
        crawling_since: running_since(running, WorkLockScope::Spider, listing_source_id, now),
        scraping_since: running_since(running, WorkLockScope::Scraper, listing_source_id, now),
        crawl_state: CrawlStateOverview {
            last_crawl_outcome: row.try_get("last_crawl_outcome")?,
            last_crawl_error_kind: row.try_get("last_crawl_error_kind")?,
            crawl_failure_count: row.try_get("crawl_failure_count")?,
            next_crawl_at: row.try_get("next_crawl_at")?,
            scrape_failure_streak: row.try_get("scrape_failure_streak")?,
            last_scrape_status_code: row.try_get("last_scrape_status_code")?,
            last_scrape_error_kind: row.try_get("last_scrape_error_kind")?,
            scrape_failing_since: row.try_get("scrape_failing_since")?,
            scrape_paused_until: row.try_get("scrape_paused_until")?,
        },
    })
}

/// When the `scope` component acquired its work lock on the source, as wall-clock time, or `None`
/// while it holds none.
fn running_since(
    running: &[RunningWork],
    scope: WorkLockScope,
    listing_source_id: ListingSourceId,
    now: OffsetDateTime,
) -> Option<OffsetDateTime> {
    running
        .iter()
        .find(|work| work.scope == scope && work.listing_source_id == listing_source_id)
        .map(|work| now - work.since.elapsed())
}

/// The actor a persisted confirmation names; an unknown name fails, so the console never shows a
/// confirmation it cannot attribute.
fn confirming_actor(name: Option<String>) -> Result<Option<ReviewActor>, sqlx::Error> {
    name.map(|name| {
        ReviewActor::from_persisted(&name).ok_or_else(|| {
            sqlx::Error::Decode(
                format!("invalid persisted mass_removal_confirmed_by `{name}`").into(),
            )
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spider::candidate_service::FAILED_CRAWL_OUTCOME;

    #[test]
    fn should_select_failed_crawls_by_the_persisted_failed_outcome() {
        assert!(
            OVERVIEW_CTES.contains(&format!("IS NOT DISTINCT FROM '{FAILED_CRAWL_OUTCOME}'")),
            "the overview SQL names the failed outcome as a literal"
        );
    }
}
