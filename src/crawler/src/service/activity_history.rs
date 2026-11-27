//! The crawler's hourly activity counts: what the scraper and the spider did in each UTC clock
//! hour, added as each scraper batch and each spider run ends, for the console's Overview. The
//! contract is in `docs/crawler/operations.md#activity-history`.

use sqlx::PgPool;
use std::sync::atomic::{AtomicI64, Ordering};
use time::OffsetDateTime;

/// How many days of hours the history keeps; older hours are deleted as the writes go on.
pub const ACTIVITY_RETENTION_DAYS: i64 = 35;

/// Counts to add to one clock hour. Every count is a number of pages, runs or URLs, never
/// negative.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActivityCounts {
    /// Product pages the scraper fetched and evaluated: captured, unchanged or removed.
    pub pages_scraped: i64,
    /// Page scrapes that failed.
    pub page_scrapes_failed: i64,
    pub crawl_runs_completed: i64,
    pub crawl_runs_failed: i64,
    /// Spider runs that ended neither completed nor failed: cut short, rate-limited, stopped by
    /// the shutdown or waiting on a URL-pattern review.
    pub crawl_runs_other: i64,
    /// Product URLs a spider run stored for the first time.
    pub product_urls_found: i64,
}

/// How a spider run ended, as the history counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlRunClass {
    /// The crawl reached everything it was to crawl.
    Completed,
    /// The run failed, including a failure that stopped its ListingSource for an operator.
    Failed,
    Other,
}

impl ActivityCounts {
    /// The counts of one scraper batch.
    pub fn scrape_batch(pages_scraped: usize, page_scrapes_failed: usize) -> Self {
        Self {
            pages_scraped: count(pages_scraped),
            page_scrapes_failed: count(page_scrapes_failed),
            ..Self::default()
        }
    }

    /// The counts of one spider run that ended as `class` and stored `product_urls_found` new
    /// product URLs.
    pub fn crawl_run(class: CrawlRunClass, product_urls_found: usize) -> Self {
        let run = |of: CrawlRunClass| i64::from(class == of);
        Self {
            crawl_runs_completed: run(CrawlRunClass::Completed),
            crawl_runs_failed: run(CrawlRunClass::Failed),
            crawl_runs_other: run(CrawlRunClass::Other),
            product_urls_found: count(product_urls_found),
            ..Self::default()
        }
    }

    /// Whether every count is zero, so writing them would change nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

fn count(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Adds counts to the hours of `crawler_activity_hours`.
pub struct ActivityHistoryRepository {
    pool: PgPool,
    /// The clock hour, in hours since the Unix epoch, whose writes last deleted the expired
    /// hours; the first write of each later hour deletes them again.
    pruned_hour: AtomicI64,
}

impl ActivityHistoryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            pruned_hour: AtomicI64::new(i64::MIN),
        }
    }

    /// Adds `counts` to the UTC clock hour `at` falls in, in one upsert. The first write of each
    /// clock hour then deletes the hours older than [`ACTIVITY_RETENTION_DAYS`] before it, so a
    /// failed deletion loses no count and is tried again in the next hour.
    pub async fn add(&self, at: OffsetDateTime, counts: ActivityCounts) -> Result<(), sqlx::Error> {
        if !counts.is_empty() {
            self.upsert(at, counts).await?;
        }
        let hour = at.unix_timestamp().div_euclid(3600);
        if self.pruned_hour.swap(hour, Ordering::Relaxed) != hour {
            self.delete_expired_hours(at).await?;
        }
        Ok(())
    }

    async fn upsert(&self, at: OffsetDateTime, counts: ActivityCounts) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO crawler_activity_hours AS a
                 (hour, pages_scraped, page_scrapes_failed, crawl_runs_completed,
                  crawl_runs_failed, crawl_runs_other, product_urls_found)
             VALUES (date_trunc('hour', $1::TIMESTAMPTZ, 'UTC'), $2, $3, $4, $5, $6, $7)
             ON CONFLICT (hour) DO UPDATE SET
                 pages_scraped = a.pages_scraped + EXCLUDED.pages_scraped,
                 page_scrapes_failed = a.page_scrapes_failed + EXCLUDED.page_scrapes_failed,
                 crawl_runs_completed = a.crawl_runs_completed + EXCLUDED.crawl_runs_completed,
                 crawl_runs_failed = a.crawl_runs_failed + EXCLUDED.crawl_runs_failed,
                 crawl_runs_other = a.crawl_runs_other + EXCLUDED.crawl_runs_other,
                 product_urls_found = a.product_urls_found + EXCLUDED.product_urls_found",
        )
        .bind(at)
        .bind(counts.pages_scraped)
        .bind(counts.page_scrapes_failed)
        .bind(counts.crawl_runs_completed)
        .bind(counts.crawl_runs_failed)
        .bind(counts.crawl_runs_other)
        .bind(counts.product_urls_found)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Deletes the hours more than [`ACTIVITY_RETENTION_DAYS`] before the clock hour of `at`, a
    /// range of the primary key.
    async fn delete_expired_hours(&self, at: OffsetDateTime) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM crawler_activity_hours
             WHERE hour < date_trunc('hour', $1::TIMESTAMPTZ, 'UTC') - make_interval(days => $2)",
        )
        .bind(at)
        .bind(i32::try_from(ACTIVITY_RETENTION_DAYS).unwrap_or(i32::MAX))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_count_a_crawl_run_once_in_the_column_of_its_class() {
        assert_eq!(
            ActivityCounts::crawl_run(CrawlRunClass::Failed, 0),
            ActivityCounts {
                crawl_runs_failed: 1,
                ..ActivityCounts::default()
            }
        );
        assert_eq!(
            ActivityCounts::crawl_run(CrawlRunClass::Completed, 12),
            ActivityCounts {
                crawl_runs_completed: 1,
                product_urls_found: 12,
                ..ActivityCounts::default()
            }
        );
    }

    #[test]
    fn should_have_nothing_to_write_for_a_batch_that_scraped_nothing() {
        assert!(ActivityCounts::scrape_batch(0, 0).is_empty());
        assert!(!ActivityCounts::scrape_batch(0, 1).is_empty());
    }
}
