//! `GET /api/source-events`: what the crawler did to ListingSources since a time, newest first. The
//! contract is in `docs/crawler/operations.md` (review service). The crawler records the state of a
//! source and when it entered it, not a history, so these are the sources' current states dated
//! within the window.

use crate::review::model::{SourceEvent, SourceEventKind};
use crate::review::repository::listing_source_id_from_uuid;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;

/// Reads what the crawler did to ListingSources.
#[derive(Clone)]
pub struct SourceEventReader {
    pool: PgPool,
}

impl SourceEventReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The events dated `since` or later, newest first, at most `limit`, and whether more were
    /// left out.
    pub async fn list_source_events(
        &self,
        since: OffsetDateTime,
        limit: usize,
    ) -> Result<(Vec<SourceEvent>, bool), sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM (
                 SELECT 'SCRAPE_FAILING' AS kind, listing_source_id, listing_source_name,
                        scrape_failing_since AS occurred, NULL::text AS outcome,
                        last_scrape_error_kind AS error_kind, scrape_failure_streak AS count
                 FROM listing_sources
                 WHERE crawl_enabled AND scrape_failure_streak > 0 AND scrape_failing_since >= $1
                 UNION ALL
                 SELECT 'CRAWL_FINISHED', listing_source_id, listing_source_name,
                        last_crawl_run_at, last_crawl_outcome, last_crawl_error_kind,
                        crawl_failure_count
                 FROM listing_sources
                 WHERE crawl_enabled AND last_crawl_outcome IS NOT NULL AND last_crawl_run_at >= $1
             ) events
             ORDER BY occurred DESC, listing_source_id, kind
             LIMIT $2",
        )
        .bind(since)
        .bind(i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        let truncated = rows.len() > limit;
        let events = rows
            .into_iter()
            .take(limit)
            .map(|row| {
                let kind: String = row.try_get("kind")?;
                Ok(SourceEvent {
                    kind: match kind.as_str() {
                        "SCRAPE_FAILING" => SourceEventKind::ScrapeFailing,
                        _ => SourceEventKind::CrawlFinished,
                    },
                    listing_source_id: listing_source_id_from_uuid(
                        row.try_get("listing_source_id")?,
                    )?,
                    listing_source_name: row.try_get("listing_source_name")?,
                    occurred: row.try_get("occurred")?,
                    outcome: row.try_get("outcome")?,
                    error_kind: row.try_get("error_kind")?,
                    count: row.try_get("count")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        Ok((events, truncated))
    }
}
