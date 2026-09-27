use std::sync::atomic::{AtomicU64, Ordering};
use tracing::info;

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
            info!(
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

    #[test]
    fn average_seconds_preserves_fractional_seconds() {
        assert!((average_seconds(1_847, 1) - 1.847).abs() < f64::EPSILON);
        assert!((average_seconds(3_694, 2) - 1.847).abs() < f64::EPSILON);
    }
}
