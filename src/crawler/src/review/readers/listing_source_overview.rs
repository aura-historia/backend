//! `GET /api/listing_sources`: the operator's ListingSource overview, page by page. The contract is
//! in `docs/crawler/operations.md` (review service).
//!
//! The ListingSources are ranked in the order the query asks for, with the id as the last key, and
//! a page starts after the ListingSource the previous one ended with, at that ListingSource's
//! place in the ranking of the moment. Every filter, the search, the sort and the count run in SQL
//! over all ListingSources.

use super::PageReadError;
use crate::review::model::{
    CrawlStateOverview, ListingSourceOverview, ListingSourcePage, PageSize, ReviewActor,
    SearchTerm, SortDirection, SourceActivity, SourceHealth, SourceSort, UrlErrorKindCount,
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

/// Which page of the ListingSource overview to read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListingSourceQuery {
    /// ListingSources whose name, slug or crawl host contains this text, or whose id is this text.
    pub search: Option<SearchTerm>,
    /// Only ListingSources this process is crawling or scraping right now.
    pub activity: Option<SourceActivity>,
    pub health: Option<SourceHealth>,
    /// Only ListingSources that are, or are not, crawl-enabled.
    pub crawl_enabled: Option<bool>,
    pub sort: SourceSort,
    pub direction: SortDirection,
    /// The ListingSource the previous page ended with; `None` reads the first page.
    pub after: Option<ListingSourceId>,
    pub page_size: PageSize,
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
        s.listing_source_slug,
        s.crawl_host,
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
        CASE WHEN s.mass_removal_confirmed_until > NOW() THEN s.mass_removal_confirmed_until END
            AS mass_removal_confirmed_until,
        CASE WHEN s.mass_removal_confirmed_until > NOW() THEN s.mass_removal_confirmed_by END
            AS mass_removal_confirmed_by,
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

    pub(super) fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// This reader reporting the crawler's running work from `work_locks`, the lock manager the
    /// spider and scraper of this process hold their work locks in.
    pub fn with_work_locks(self, work_locks: Arc<LocalLockManager>) -> Self {
        Self { work_locks, ..self }
    }

    /// The page of ListingSources `query` asks for, with how many match it in all. A ListingSource
    /// that `query.after` names but that does not exist is [`PageReadError::UnknownCursor`].
    pub async fn list_listing_sources(
        &self,
        query: &ListingSourceQuery,
    ) -> Result<ListingSourcePage, PageReadError> {
        if let Some(after) = query.after {
            self.ensure_exists(after).await?;
        }
        let page_size = query.page_size.get();
        let mut listing_sources = self.read_overviews(query, None).await?;
        let total = listing_sources.total;
        let next_after = if listing_sources.rows.len() > page_size {
            listing_sources.rows.truncate(page_size);
            listing_sources
                .rows
                .last()
                .map(|overview| overview.listing_source_id)
        } else {
            None
        };
        Ok(ListingSourcePage {
            listing_sources: listing_sources.rows,
            total,
            next_after,
        })
    }

    /// The overview of `listing_source_id`, or `None` when no such ListingSource exists.
    pub async fn find_listing_source(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<Option<ListingSourceOverview>, sqlx::Error> {
        let query = ListingSourceQuery {
            page_size: PageSize::new(1).unwrap_or_default(),
            ..ListingSourceQuery::default()
        };
        Ok(self
            .read_overviews(&query, Some(listing_source_id))
            .await?
            .rows
            .pop())
    }

    async fn ensure_exists(&self, listing_source_id: ListingSourceId) -> Result<(), PageReadError> {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM listing_sources WHERE listing_source_id = $1)",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_one(&self.pool)
        .await?;
        if exists {
            Ok(())
        } else {
            Err(PageReadError::UnknownCursor(listing_source_id.to_string()))
        }
    }

    /// The overviews `query` asks for, one more than its page size when more follow, and how
    /// many match it in all, with the URL error kinds of each read in one more query. `only`
    /// restricts the read to that ListingSource.
    async fn read_overviews(
        &self,
        query: &ListingSourceQuery,
        only: Option<ListingSourceId>,
    ) -> Result<OverviewRows, sqlx::Error> {
        let running = self.work_locks.running_work();
        let running_ids = |scope| {
            running
                .iter()
                .filter(|work| work.scope == scope)
                .map(|work| uuid::Uuid::from(work.listing_source_id))
                .collect::<Vec<_>>()
        };
        let now = OffsetDateTime::now_utc();
        let page_size = query.page_size.get();
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
        .bind(
            query
                .search
                .as_ref()
                .and_then(|term| term.as_str().parse::<ListingSourceId>().ok())
                .map(|id| *id.as_uuid()),
        )
        .bind(query.activity.map(SourceActivity::as_str))
        .bind(query.health.map(SourceHealth::as_str))
        .bind(query.crawl_enabled)
        .bind(query.after.map(|id| *id.as_uuid()))
        .bind(i64::try_from(page_size.saturating_add(1)).unwrap_or(i64::MAX))
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
        Ok(OverviewRows {
            rows: listing_sources,
            total,
        })
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

/// The overviews a read found and how many ListingSources match its query in all.
struct OverviewRows {
    rows: Vec<ListingSourceOverview>,
    total: i64,
}

/// The ListingSources of `OVERVIEW_CTES` ranked by `sort` in `direction`, filtered, and one page of
/// them after a ListingSource. Binds `$1` to `$3` of [`OVERVIEW_CTES`], `$4` and `$5`, the
/// ListingSources the spider and the scraper of this process work right now, `$6`, the search's
/// `ILIKE` pattern or null, `$7`, the ListingSource id the search names or null, `$8`, the
/// activity filter or null, `$9`, the health filter or null, `$10`, the crawl-enabled filter or
/// null, `$11`, the ListingSource the page starts after or null, and `$12`, the most rows to
/// answer. Every row carries the total; an empty page is one row with the total and no
/// ListingSource.
///
/// The SQL is assembled from static fragments only, chosen by `sort` and `direction`, never from
/// request text.
fn overview_page_sql(sort: SourceSort, direction: SortDirection) -> String {
    format!(
        "{OVERVIEW_CTES}
         ), ranked AS (
             SELECT overview.*,
                    crawl_enabled AND (needs_operator OR blocked_urls > 0
                                       OR scrape_failure_streak > 0
                                       OR held_removal_urls > 0) AS needs_attention,
                    ROW_NUMBER() OVER (ORDER BY {order}) AS position
             FROM overview
         ), matching AS (
             SELECT * FROM ranked
             WHERE ($6::text IS NULL
                    OR listing_source_name ILIKE $6
                    OR listing_source_slug ILIKE $6
                    OR crawl_host ILIKE $6
                    OR listing_source_id = $7::uuid)
               AND ($8::text IS NULL
                    OR ($8 IN ('crawling', 'active') AND listing_source_id = ANY($4))
                    OR ($8 IN ('scraping', 'active') AND listing_source_id = ANY($5)))
               AND ($9::text IS NULL
                    OR ($9 = 'needs_attention' AND needs_attention)
                    OR ($9 = 'needs_operator' AND crawl_enabled AND needs_operator)
                    OR ($9 = 'blocked' AND crawl_enabled AND blocked_urls > 0)
                    OR ($9 = 'scrape_failing' AND crawl_enabled AND scrape_failure_streak > 0)
                    OR ($9 = 'held_removals' AND crawl_enabled AND held_removal_urls > 0)
                    OR ($9 = 'healthy' AND crawl_enabled AND NOT needs_attention))
               AND ($10::boolean IS NULL OR crawl_enabled = $10)
         ), page AS (
             SELECT * FROM matching
             WHERE position > COALESCE(
                 (SELECT position FROM ranked WHERE listing_source_id = $11), 0)
             ORDER BY position
             LIMIT $12
         )
         SELECT t.total, p.*
         FROM (SELECT COUNT(*) AS total FROM matching) t
         LEFT JOIN page p ON TRUE
         ORDER BY p.position",
        order = order_by(sort, direction),
    )
}

/// The `ORDER BY` list of `sort` in `direction`: every key in that direction, nulls last, the
/// ListingSource id last. `Attention` ranks crawl-enabled sources first, then those whose crawl
/// waits for an operator, those with blocked URLs, the longest scrape failure streaks, those with
/// held removals, the most pending reviews and the most recently updated.
fn order_by(sort: SourceSort, direction: SortDirection) -> String {
    let keys: &[&str] = match sort {
        SourceSort::Attention => &[
            "crawl_enabled",
            "needs_operator",
            "(blocked_urls > 0)",
            "scrape_failure_streak",
            "(held_removal_urls > 0)",
            "pending_reviews",
            "updated",
        ],
        SourceSort::ListingSourceName => &["lower(listing_source_name)"],
        SourceSort::PendingReviews => &["pending_reviews"],
        SourceSort::ProductUrls => &["product_urls"],
        SourceSort::BlockedUrls => &["blocked_urls"],
        SourceSort::HeldRemovalUrls => &["held_removal_urls"],
        SourceSort::ScrapeFailureStreak => &["scrape_failure_streak"],
        SourceSort::LlmCallsCount => &["llm_calls_count"],
        SourceSort::LastSuccessfulCrawl => &["last_successful_crawl"],
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
        listing_source_slug: row.try_get("listing_source_slug")?,
        crawl_enabled: row.try_get("crawl_enabled")?,
        needs_attention: row.try_get("needs_attention")?,
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
        mass_removal_confirmed_until: row.try_get("mass_removal_confirmed_until")?,
        mass_removal_confirmed_by: confirming_actor(row.try_get("mass_removal_confirmed_by")?)?,
        url_error_kinds: Vec::new(),
        missing_value_urls: row.try_get("missing_value_urls")?,
        crawling_since: running_since(running, WorkLockScope::Spider, listing_source_id, now),
        scraping_since: running_since(running, WorkLockScope::Scraper, listing_source_id, now),
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
