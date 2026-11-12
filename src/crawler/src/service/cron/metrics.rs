use crate::metrics::{CrawlerMetric, MetricRecord};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::debug;

pub(super) struct PerfCounter {
    count: AtomicU64,
    duration_ms: AtomicU64,
    threshold: u64,
    label: &'static str,
}

impl Clone for PerfCounter {
    fn clone(&self) -> Self {
        Self {
            count: AtomicU64::new(self.count.load(Ordering::Relaxed)),
            duration_ms: AtomicU64::new(self.duration_ms.load(Ordering::Relaxed)),
            threshold: self.threshold,
            label: self.label,
        }
    }
}

impl PerfCounter {
    pub(super) fn new(threshold: u64, label: &'static str) -> Self {
        Self {
            count: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
            threshold,
            label,
        }
    }

    pub(super) fn record(&self, count: u64, duration_ms: u64) {
        if count == 0 {
            return;
        }

        self.count.fetch_add(count, Ordering::Relaxed);
        self.duration_ms.fetch_add(duration_ms, Ordering::Relaxed);

        let total = self.count.load(Ordering::Relaxed);
        if total >= self.threshold {
            let total_ms = self.duration_ms.load(Ordering::Relaxed);
            let avg_seconds = average_seconds(total_ms, total);
            debug!(
                items_processed = total,
                avg_seconds,
                label = self.label,
                "Performance summary"
            );
            self.count.store(0, Ordering::Relaxed);
            self.duration_ms.store(0, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> (u64, u64) {
        (
            self.count.load(Ordering::Relaxed),
            self.duration_ms.load(Ordering::Relaxed),
        )
    }
}

fn average_seconds(total_ms: u64, total: u64) -> f64 {
    total_ms as f64 / total as f64 / 1000.0
}

/// Publishes the totals of a scheduler pass and how long it took, as `duration_metric`, in one
/// record, and returns the duration in milliseconds for the pass summary. Every pass publishes
/// exactly one record, also a pass that finds no work.
pub(super) fn emit_pass_record(
    totals: MetricRecord,
    duration_metric: CrawlerMetric,
    pass_start: tokio::time::Instant,
) -> u64 {
    let duration_ms = u64::try_from(pass_start.elapsed().as_millis()).unwrap_or(u64::MAX);
    totals.with_value(duration_metric, duration_ms).emit();
    duration_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_records_do_nothing() {
        let counter = PerfCounter::new(500, "test");

        counter.record(0, 10_000);

        assert_eq!(counter.snapshot(), (0, 0));
    }

    #[test]
    fn positive_records_accumulate_before_threshold() {
        let counter = PerfCounter::new(500, "test");

        counter.record(3, 300);

        assert_eq!(counter.snapshot(), (3, 300));
    }

    #[test]
    fn threshold_resets_after_summary() {
        let counter = PerfCounter::new(3, "test");

        counter.record(2, 200);
        assert_eq!(counter.snapshot(), (2, 200));

        counter.record(1, 100);

        assert_eq!(counter.snapshot(), (0, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn should_publish_the_pass_totals_and_duration_in_one_record() {
        let (metrics, _guard) = crate::metrics::test_support::capture_metrics();
        let pass_start = tokio::time::Instant::now();
        tokio::time::advance(std::time::Duration::from_millis(1_500)).await;

        let duration_ms = emit_pass_record(
            MetricRecord::default().with_value(CrawlerMetric::CrawlRunsFailed, 2),
            CrawlerMetric::SpiderPassDuration,
            pass_start,
        );

        assert_eq!(duration_ms, 1_500);
        let records = metrics.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["CrawlRunsFailed"], 2);
        assert_eq!(records[0]["SpiderPassDuration"], 1_500);
    }

    #[test]
    fn average_seconds_preserves_fractional_seconds() {
        assert!((average_seconds(1_847, 1) - 1.847).abs() < f64::EPSILON);
        assert!((average_seconds(3_694, 2) - 1.847).abs() < f64::EPSILON);
    }
}
