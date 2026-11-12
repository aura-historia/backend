//! The crawler's Postgres pools and the usage they report
//! ([operations](../../../../docs/crawler/operations.md#postgres-pools)).
//!
//! Spiders, scrapers and raw capture share the crawl pool; the review console has its own small
//! pool on the same database, so console reads cannot take the connections crawling waits for,
//! and saturated crawling cannot time out the console.

use sqlx::PgPool;
use std::time::Duration;
use tracing::{debug, info, warn};

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

/// Logs `window` with the pool's numbers, so a slow-acquire or `PoolTimedOut` log line can be read
/// against them: WARN when every connection was in use at a sample, INFO while the pool was in use,
/// DEBUG while it was idle. It is a log event and no metric, because every metric, and every
/// value of its dimension, is billed.
fn report_window(kind: DbPoolKind, max_connections: u32, window: PoolUsageWindow) {
    macro_rules! log_usage {
        ($log:ident, $message:literal) => {
            $log!(
                db_pool = kind.as_str(),
                max_connections,
                in_use_peak = window.in_use_peak,
                saturated_samples = window.saturated_samples,
                samples = window.samples,
                $message
            )
        };
    }
    if window.saturated_samples > 0 {
        log_usage!(warn, "Crawler Postgres pool had every connection in use");
    } else if window.in_use_peak > 0 {
        log_usage!(info, "Crawler Postgres pool usage");
    } else {
        log_usage!(debug, "Crawler Postgres pool was idle");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::logged_events::LoggedEvents;
    use tracing::Level;

    #[tokio::test]
    async fn should_open_the_review_pool_on_the_crawl_pools_database_and_settings() {
        let crawl = sqlx::postgres::PgPoolOptions::new()
            .max_connections(150)
            .acquire_timeout(Duration::from_secs(30))
            .connect_lazy("postgres://crawler:secret@db.example:5433/crawler?sslmode=require")
            .unwrap();

        let pools = CrawlerPools::beside(crawl);

        let review = pools.review.connect_options();
        assert_eq!(review.get_host(), "db.example");
        assert_eq!(review.get_port(), 5433);
        assert_eq!(review.get_database(), Some("crawler"));
        assert!(matches!(
            review.get_ssl_mode(),
            sqlx::postgres::PgSslMode::Require
        ));
        let options = pools.review.options();
        assert_eq!(options.get_max_connections(), REVIEW_DB_MAX_CONNECTIONS);
        assert_eq!(options.get_min_connections(), 0);
        assert_eq!(options.get_acquire_timeout(), Duration::from_secs(30));
        assert_eq!(
            pools.review.size(),
            0,
            "no connection opens before first use"
        );
        assert_eq!(pools.crawl.options().get_max_connections(), 150);
    }

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

    fn reported(
        kind: DbPoolKind,
        max_connections: u32,
        window: PoolUsageWindow,
    ) -> Vec<(Level, String)> {
        let (events, _guard) = LoggedEvents::capture();
        report_window(kind, max_connections, window);
        events
            .all()
            .into_iter()
            .map(|event| {
                assert!(
                    event.field("emf").is_none(),
                    "a pool report is no metric record"
                );
                (
                    event.level,
                    format!(
                        "{} {}/{} {}",
                        event.field("db_pool").unwrap_or_default(),
                        event.field("in_use_peak").unwrap_or_default(),
                        event.field("max_connections").unwrap_or_default(),
                        event.field("saturated_samples").unwrap_or_default(),
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn should_warn_with_the_pool_numbers_and_publish_no_metric_when_every_connection_was_in_use() {
        let window = PoolUsageWindow {
            samples: 60,
            in_use_peak: 8,
            saturated_samples: 3,
        };

        assert_eq!(
            reported(DbPoolKind::Review, 8, window),
            [(Level::WARN, "review 8/8 3".to_owned())]
        );
    }

    #[test]
    fn should_log_usage_at_info_while_the_pool_is_used_and_at_debug_while_it_is_idle() {
        let busy = PoolUsageWindow {
            samples: 60,
            in_use_peak: 4,
            saturated_samples: 0,
        };
        let idle = PoolUsageWindow {
            samples: 60,
            ..PoolUsageWindow::default()
        };

        assert_eq!(
            reported(DbPoolKind::Crawl, 150, busy),
            [(Level::INFO, "crawl 4/150 0".to_owned())]
        );
        assert_eq!(
            reported(DbPoolKind::Crawl, 150, idle),
            [(Level::DEBUG, "crawl 0/150 0".to_owned())]
        );
    }
}
