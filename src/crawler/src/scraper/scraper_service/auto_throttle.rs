use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const DEFAULT_TARGET_CONCURRENCY: f64 = 2.0;
const DEFAULT_ALPHA: f64 = 0.15;
const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(10);
const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScraperAutoThrottleConfig {
    pub target_concurrency: f64,
    pub min_delay: Duration,
    pub max_delay: Duration,
    pub alpha: f64,
    pub enabled: bool,
}

impl Default for ScraperAutoThrottleConfig {
    fn default() -> Self {
        Self {
            target_concurrency: DEFAULT_TARGET_CONCURRENCY,
            min_delay: Duration::from_secs(1),
            max_delay: DEFAULT_MAX_DELAY,
            alpha: DEFAULT_ALPHA,
            enabled: true,
        }
    }
}

impl ScraperAutoThrottleConfig {
    pub fn with_min_delay(min_delay: Duration) -> Self {
        Self {
            min_delay,
            ..Default::default()
        }
    }
}

struct DomainLatency {
    ema_us: AtomicU64,
    samples: AtomicU64,
    last_access: AtomicU64,
}

impl DomainLatency {
    fn new(access_counter: u64) -> Self {
        Self {
            ema_us: AtomicU64::new(0),
            samples: AtomicU64::new(0),
            last_access: AtomicU64::new(access_counter),
        }
    }

    fn ema_micros(&self) -> f64 {
        f64::from_bits(self.ema_us.load(Ordering::Relaxed))
    }

    fn record(&self, latency_us: f64, alpha: f64) {
        let previous_count = self.samples.fetch_add(1, Ordering::Relaxed);
        if previous_count == 0 {
            self.ema_us.store(latency_us.to_bits(), Ordering::Relaxed);
            return;
        }

        let _ = self
            .ema_us
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bits| {
                let previous = f64::from_bits(bits);
                let next = previous + alpha * (latency_us - previous);
                if next.is_finite() && next >= 0.0 {
                    Some(next.to_bits())
                } else {
                    Some(previous.to_bits())
                }
            });
    }
}

pub struct ScraperAutoThrottle {
    domains: DashMap<String, DomainLatency>,
    config: ScraperAutoThrottleConfig,
    access_counter: AtomicU64,
}

impl ScraperAutoThrottle {
    pub fn new(config: ScraperAutoThrottleConfig) -> Self {
        Self {
            domains: DashMap::with_capacity(64),
            config,
            access_counter: AtomicU64::new(0),
        }
    }

    pub fn record_latency(&self, domain: &str, latency: Duration) {
        if domain.is_empty() {
            return;
        }

        let latency_us = latency.as_micros() as f64;
        let access_counter = self.access_counter.fetch_add(1, Ordering::Relaxed);
        let alpha = self.config.alpha.clamp(0.01, 1.0);

        if let Some(entry) = self.domains.get(domain) {
            entry.last_access.store(access_counter, Ordering::Relaxed);
            entry.record(latency_us, alpha);
            return;
        }

        self.maybe_evict();
        let entry = DomainLatency::new(access_counter);
        entry.record(latency_us, alpha);
        self.domains.insert(domain.to_string(), entry);
    }

    pub fn delay_for(&self, domain: &str) -> Duration {
        if !self.config.enabled {
            return Duration::ZERO;
        }

        let Some(entry) = self.domains.get(domain) else {
            return self.config.min_delay;
        };

        if entry.samples.load(Ordering::Relaxed) == 0 {
            return self.config.min_delay;
        }

        let target = self.config.target_concurrency.max(0.1);
        let delay_us = entry.ema_micros() / target;
        let delay = Duration::from_micros(delay_us.max(0.0) as u64);

        delay.clamp(self.config.min_delay, self.config.max_delay)
    }

    pub fn latency_ms(&self, domain: &str) -> Option<f64> {
        self.domains.get(domain).and_then(|entry| {
            if entry.samples.load(Ordering::Relaxed) == 0 {
                None
            } else {
                Some(entry.ema_micros() / 1000.0)
            }
        })
    }

    fn maybe_evict(&self) {
        if self.domains.len() < MAX_ENTRIES {
            return;
        }

        let mut oldest_key: Option<String> = None;
        let mut oldest_access = u64::MAX;
        for entry in self.domains.iter() {
            let access = entry.value().last_access.load(Ordering::Relaxed);
            if access < oldest_access {
                oldest_access = access;
                oldest_key = Some(entry.key().clone());
            }
        }

        if let Some(key) = oldest_key {
            self.domains.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_start_returns_min_delay() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            min_delay: Duration::from_millis(250),
            ..Default::default()
        });

        assert_eq!(
            throttle.delay_for("example.com"),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn records_first_latency_sample() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            min_delay: Duration::ZERO,
            ..Default::default()
        });

        throttle.record_latency("example.com", Duration::from_millis(200));

        let latency = throttle.latency_ms("example.com").unwrap();
        assert!((latency - 200.0).abs() < 1.0);
    }

    #[test]
    fn delay_uses_latency_over_target_concurrency() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            target_concurrency: 4.0,
            min_delay: Duration::ZERO,
            max_delay: Duration::from_secs(60),
            ..Default::default()
        });

        throttle.record_latency("example.com", Duration::from_millis(400));

        assert_eq!(
            throttle.delay_for("example.com"),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn delay_is_clamped_to_min_and_max() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            target_concurrency: 1.0,
            min_delay: Duration::from_millis(50),
            max_delay: Duration::from_millis(500),
            ..Default::default()
        });

        throttle.record_latency("fast.com", Duration::from_millis(5));
        throttle.record_latency("slow.com", Duration::from_secs(2));

        assert_eq!(throttle.delay_for("fast.com"), Duration::from_millis(50));
        assert_eq!(throttle.delay_for("slow.com"), Duration::from_millis(500));
    }

    #[test]
    fn domains_are_independent() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            min_delay: Duration::ZERO,
            ..Default::default()
        });

        throttle.record_latency("a.com", Duration::from_millis(100));
        throttle.record_latency("b.com", Duration::from_millis(1000));

        assert!(throttle.delay_for("a.com") < throttle.delay_for("b.com"));
    }
}
