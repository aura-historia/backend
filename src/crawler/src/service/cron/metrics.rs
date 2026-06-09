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
        self.count.fetch_add(count, Ordering::Relaxed);
        self.duration_ms.fetch_add(duration_ms, Ordering::Relaxed);

        let total = self.count.load(Ordering::Relaxed);
        if total >= self.threshold {
            let total_ms = self.duration_ms.load(Ordering::Relaxed);
            let avg_ms = total_ms / total;
            info!(
                items_processed = total,
                avg_ms = avg_ms,
                label = self.label,
                "Performance summary"
            );
            self.count.store(0, Ordering::Relaxed);
            self.duration_ms.store(0, Ordering::Relaxed);
        }
    }
}
