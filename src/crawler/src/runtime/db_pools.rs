//! The crawler's Postgres pools and the usage they report
//! ([operations](../../../../docs/crawler/operations.md#postgres-pools)).
//!
//! Spiders, scrapers and raw capture share the crawl pool; the review console has its own small
//! pool on the same database, so console reads cannot take the connections crawling waits for,
//! and saturated crawling cannot time out the console.

use crate::metrics::{CrawlerMetric, MetricDimension, MetricRecord};
use sqlx::PgPool;
use std::time::Duration;
use tracing::warn;

/// Connections the review console's pool opens at most.
pub const REVIEW_DB_MAX_CONNECTIONS: u32 = 8;
const USAGE_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// Samples per usage report, one report a minute.
const USAGE_SAMPLES_PER_REPORT: u64 = 60;

/// Which of the crawler's pools a usage report describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DbPoolKind {
    Crawl,
    Review,
}

impl DbPoolKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Crawl => "crawl",
            Self::Review => "review",
        }
    }
}

/// The crawl pool the entry point connected and the review console's pool beside it.
#[derive(Clone)]
pub(super) struct CrawlerPools {
    pub(super) crawl: PgPool,
    pub(super) review: PgPool,
}

impl CrawlerPools {
    /// Adds the review console's pool to `crawl`: the same database and pool settings, at most
    /// [`REVIEW_DB_MAX_CONNECTIONS`] connections, each opened on first use.
    pub(super) fn beside(crawl: PgPool) -> Self {
        let review = crawl
            .options()
            .clone()
            .min_connections(0)
            .max_connections(REVIEW_DB_MAX_CONNECTIONS)
            .connect_lazy_with((*crawl.connect_options()).clone());
        Self { crawl, review }
    }

    /// Samples both pools every second and reports each once a minute, until dropped.
    pub(super) async fn report_usage(self) {
        let mut monitors = [
            PoolMonitor::new(DbPoolKind::Crawl, self.crawl),
            PoolMonitor::new(DbPoolKind::Review, self.review),
        ];
        let mut ticker = tokio::time::interval(USAGE_SAMPLE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            for monitor in &mut monitors {
                monitor.sample();
            }
            if monitors[0].window.samples >= USAGE_SAMPLES_PER_REPORT {
                for monitor in &mut monitors {
                    monitor.report();
                }
            }
        }
    }
}

struct PoolMonitor {
    kind: DbPoolKind,
    pool: PgPool,
    window: PoolUsageWindow,
}

impl PoolMonitor {
    fn new(kind: DbPoolKind, pool: PgPool) -> Self {
        Self {
            kind,
            pool,
            window: PoolUsageWindow::default(),
        }
    }

    fn max_connections(&self) -> u32 {
        self.pool.options().get_max_connections()
    }

    fn sample(&mut self) {
        let idle = u32::try_from(self.pool.num_idle()).unwrap_or(u32::MAX);
        let in_use = self.pool.size().saturating_sub(idle);
        self.window.record(in_use, self.max_connections());
    }

    fn report(&mut self) {
        let window = std::mem::take(&mut self.window);
        report_window(self.kind, self.max_connections(), window);
    }
}

/// What the samples of one report interval saw of a pool.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PoolUsageWindow {
    samples: u64,
    in_use_peak: u32,
    saturated_samples: u64,
}

impl PoolUsageWindow {
    fn record(&mut self, in_use: u32, max_connections: u32) {
        self.samples += 1;
        self.in_use_peak = self.in_use_peak.max(in_use);
        if in_use >= max_connections {
            self.saturated_samples += 1;
        }
    }
}

/// Publishes `window` as metrics, and warns with the pool's numbers when every connection was in
/// use at a sample, so a slow-acquire or `PoolTimedOut` log line can be read against them.
fn report_window(kind: DbPoolKind, max_connections: u32, window: PoolUsageWindow) {
    MetricRecord::global()
        .with_dimension(MetricDimension::DbPool, kind.as_str())
        .with_value(
            CrawlerMetric::DbPoolMaxConnections,
            u64::from(max_connections),
        )
        .with_value(
            CrawlerMetric::DbPoolInUsePeak,
            u64::from(window.in_use_peak),
        )
        .with_value(
            CrawlerMetric::DbPoolSaturatedSamples,
            window.saturated_samples,
        )
        .emit();
    if window.saturated_samples > 0 {
        warn!(
            db_pool = kind.as_str(),
            max_connections,
            in_use_peak = window.in_use_peak,
            saturated_samples = window.saturated_samples,
            samples = window.samples,
            "Crawler Postgres pool had every connection in use"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::test_support::capture_metrics;

    #[test]
    fn should_keep_the_peak_and_count_saturated_samples_when_sampling_a_pool() {
        let mut window = PoolUsageWindow::default();
        for in_use in [3, 10, 7, 10, 0] {
            window.record(in_use, 10);
        }

        assert_eq!(
            window,
            PoolUsageWindow {
                samples: 5,
                in_use_peak: 10,
                saturated_samples: 2,
            }
        );
    }

    #[test]
    fn should_publish_pool_usage_under_the_pool_dimension_when_reporting_a_window() {
        let (metrics, _guard) = capture_metrics();
        let window = PoolUsageWindow {
            samples: 60,
            in_use_peak: 8,
            saturated_samples: 3,
        };

        report_window(DbPoolKind::Review, 8, window);

        let review = |metric| metrics.values_with(metric, MetricDimension::DbPool, "review");
        assert_eq!(review(CrawlerMetric::DbPoolMaxConnections), [8]);
        assert_eq!(review(CrawlerMetric::DbPoolInUsePeak), [8]);
        assert_eq!(review(CrawlerMetric::DbPoolSaturatedSamples), [3]);
    }
}
