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
    /// Product pages the scraper fetched and evaluated without a failure: the sum of the pages
    /// by [`ScrapedPageOutcome`] below.
    pub pages_scraped: i64,
    pub pages_new: i64,
    pub pages_changed: i64,
    pub pages_unchanged: i64,
    pub pages_removed: i64,
    pub pages_skipped: i64,
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

/// What a page the scraper fetched and evaluated without a failure came to, as the history
/// counts it. A page that waits on a product-schema review is neither scraped nor failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrapedPageOutcome {
    /// The page's listing captured for the first time: its URL holds no capture yet.
    New,
    /// The page's listing captured again: its raw input differs from the URL's last capture, or
    /// the page moved to another URL.
    Changed,
    /// Nothing new to capture: the page, or the removal it still shows, matches the URL's last
    /// capture.
    Unchanged,
    /// A verified removal captured: a removed-page template, a gone page or other removal
    /// evidence. A sold page is no removal: it is captured, and the worker reads its state.
    Removed,
    /// Scraped, and nothing captured for a reason the URL's outcome log names: a removal the
    /// mass-removal guard held, a duplicate URL retired, a redirect nothing settles or a write
    /// another writer overtook.
    Skipped,
}

/// The pages of one scraper batch by [`ScrapedPageOutcome`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrapedPages {
    pub new: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub skipped: usize,
}

impl ScrapedPages {
    pub fn record(&mut self, outcome: ScrapedPageOutcome) {
        let count = match outcome {
            ScrapedPageOutcome::New => &mut self.new,
            ScrapedPageOutcome::Changed => &mut self.changed,
            ScrapedPageOutcome::Unchanged => &mut self.unchanged,
            ScrapedPageOutcome::Removed => &mut self.removed,
            ScrapedPageOutcome::Skipped => &mut self.skipped,
        };
        *count += 1;
    }

    /// Every page scraped, whatever it came to.
    pub fn total(&self) -> usize {
        self.new + self.changed + self.unchanged + self.removed + self.skipped
    }
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
    pub fn scrape_batch(pages: ScrapedPages, page_scrapes_failed: usize) -> Self {
        Self {
            pages_scraped: count(pages.total()),
            pages_new: count(pages.new),
            pages_changed: count(pages.changed),
            pages_unchanged: count(pages.unchanged),
            pages_removed: count(pages.removed),
            pages_skipped: count(pages.skipped),
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
                  crawl_runs_failed, crawl_runs_other, product_urls_found, pages_new,
                  pages_changed, pages_unchanged, pages_removed, pages_skipped)
             VALUES (date_trunc('hour', $1::TIMESTAMPTZ, 'UTC'), $2, $3, $4, $5, $6, $7, $8, $9,
                     $10, $11, $12)
             ON CONFLICT (hour) DO UPDATE SET
                 pages_scraped = a.pages_scraped + EXCLUDED.pages_scraped,
                 pages_new = a.pages_new + EXCLUDED.pages_new,
                 pages_changed = a.pages_changed + EXCLUDED.pages_changed,
                 pages_unchanged = a.pages_unchanged + EXCLUDED.pages_unchanged,
                 pages_removed = a.pages_removed + EXCLUDED.pages_removed,
                 pages_skipped = a.pages_skipped + EXCLUDED.pages_skipped,
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
        .bind(counts.pages_new)
        .bind(counts.pages_changed)
        .bind(counts.pages_unchanged)
        .bind(counts.pages_removed)
        .bind(counts.pages_skipped)
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
        assert!(ActivityCounts::scrape_batch(ScrapedPages::default(), 0).is_empty());
        assert!(!ActivityCounts::scrape_batch(ScrapedPages::default(), 1).is_empty());
    }

    /// The pages scraped stay the total the Overview showed before the split, now the sum of
    /// the pages by outcome.
    #[test]
    fn should_count_the_pages_of_a_batch_by_outcome_and_as_their_sum() {
        let mut pages = ScrapedPages::default();
        for outcome in [
            ScrapedPageOutcome::New,
            ScrapedPageOutcome::New,
            ScrapedPageOutcome::Changed,
            ScrapedPageOutcome::Unchanged,
            ScrapedPageOutcome::Unchanged,
            ScrapedPageOutcome::Unchanged,
            ScrapedPageOutcome::Removed,
            ScrapedPageOutcome::Skipped,
        ] {
            pages.record(outcome);
        }

        assert_eq!(
            ActivityCounts::scrape_batch(pages, 4),
            ActivityCounts {
                pages_scraped: 8,
                pages_new: 2,
                pages_changed: 1,
                pages_unchanged: 3,
                pages_removed: 1,
                pages_skipped: 1,
                page_scrapes_failed: 4,
                ..ActivityCounts::default()
            }
        );
    }
}
