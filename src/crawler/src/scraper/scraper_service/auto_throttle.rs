//! Per-domain scraper pacing. Two rules share one per-domain entry:
//!
//! - the latency throttle: a delay before each request, derived from the
//!   domain's response latency and clamped to the configured minimum and
//!   maximum;
//! - the robots.txt `Crawl-delay`: a minimum spacing between request starts.
//!   It is measured from the previous request rather than slept in full, so a
//!   domain's first request (a review live inspection, say) is not held back.

use dashmap::DashMap;
use dashmap::mapref::one::RefMut;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::Instant;

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
    crawl_delay: Mutex<CrawlDelayClock>,
}

#[derive(Default)]
struct CrawlDelayClock {
    crawl_delay: Option<Duration>,
    last_request_at: Option<Instant>,
}

impl DomainLatency {
    fn new(access_counter: u64) -> Self {
        Self {
            ema_us: AtomicU64::new(0),
            samples: AtomicU64::new(0),
            last_access: AtomicU64::new(access_counter),
            crawl_delay: Mutex::new(CrawlDelayClock::default()),
        }
    }

    fn crawl_delay_clock(&self) -> std::sync::MutexGuard<'_, CrawlDelayClock> {
        self.crawl_delay
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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

    fn record_failure(&self, latency_us: f64, alpha: f64) {
        let previous_count = self.samples.fetch_add(1, Ordering::Relaxed);
        if previous_count == 0 {
            self.ema_us.store(latency_us.to_bits(), Ordering::Relaxed);
            return;
        }

        let _ = self
            .ema_us
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bits| {
                let previous = f64::from_bits(bits);
                let calculated = previous + alpha * (latency_us - previous);
                let next = calculated.max(previous);
                Some(next.to_bits())
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
        self.record_success_latency(domain, latency);
    }

    pub fn record_success_latency(&self, domain: &str, latency: Duration) {
        self.record_with(domain, latency, false);
    }

    pub fn record_failure_latency(&self, domain: &str, latency: Duration) {
        self.record_with(domain, latency, true);
    }

    fn record_with(&self, raw_domain: &str, latency: Duration, failure: bool) {
        let latency_us = latency.as_micros() as f64;
        let alpha = self.config.alpha.clamp(0.01, 1.0);
        if let Some(entry) = self.entry(raw_domain) {
            if failure {
                entry.record_failure(latency_us, alpha);
            } else {
                entry.record(latency_us, alpha);
            }
        }
    }

    /// The domain's entry, created on first use and marked as accessed.
    fn entry(&self, raw_domain: &str) -> Option<RefMut<'_, String, DomainLatency>> {
        let domain = crate::network::policy::canonical_crawler_domain(raw_domain);
        if domain.is_empty() {
            return None;
        }
        let access_counter = self.access_counter.fetch_add(1, Ordering::Relaxed);
        if !self.domains.contains_key(&domain) {
            self.maybe_evict();
        }
        let entry = self
            .domains
            .entry(domain)
            .or_insert_with(|| DomainLatency::new(access_counter));
        entry.last_access.store(access_counter, Ordering::Relaxed);
        Some(entry)
    }

    /// Sets the domain's robots.txt `Crawl-delay`; `None` clears it.
    pub fn set_crawl_delay(&self, domain: &str, crawl_delay: Option<Duration>) {
        if let Some(entry) = self.entry(domain) {
            entry.crawl_delay_clock().crawl_delay = crawl_delay;
        }
    }

    /// Marks the start of a request to `domain`; the crawl delay is measured
    /// from here.
    pub fn record_request_start(&self, domain: &str) {
        if let Some(entry) = self.entry(domain) {
            entry.crawl_delay_clock().last_request_at = Some(Instant::now());
        }
    }

    /// The part of the domain's crawl delay that has not yet passed since its
    /// last request start. Applies even when the latency throttle is
    /// disabled: robots.txt is not optional.
    pub fn crawl_delay_remaining(&self, domain: &str) -> Duration {
        let domain = crate::network::policy::canonical_crawler_domain(domain);
        let Some(entry) = self.domains.get(&domain) else {
            return Duration::ZERO;
        };
        let clock = entry.crawl_delay_clock();
        match (clock.crawl_delay, clock.last_request_at) {
            (Some(delay), Some(last)) => delay.saturating_sub(last.elapsed()),
            _ => Duration::ZERO,
        }
    }

    pub fn delay_for(&self, domain: &str) -> Duration {
        if !self.config.enabled {
            return Duration::ZERO;
        }

        let domain = crate::network::policy::canonical_crawler_domain(domain);
        let Some(entry) = self.domains.get(&domain) else {
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
        let domain = crate::network::policy::canonical_crawler_domain(domain);
        self.domains.get(&domain).and_then(|entry| {
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

    #[test]
    fn fast_failure_cannot_make_a_domain_look_faster() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            target_concurrency: 1.0,
            min_delay: Duration::ZERO,
            max_delay: Duration::from_secs(60),
            ..Default::default()
        });

        throttle.record_success_latency("example.com", Duration::from_secs(1));
        let before_failure = throttle.delay_for("example.com");
        throttle.record_failure_latency("example.com", Duration::from_millis(50));

        assert!(throttle.delay_for("example.com") >= before_failure);
    }

    #[tokio::test(start_paused = true)]
    async fn crawl_delay_spaces_request_starts_across_bare_and_www_hosts() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            enabled: false,
            ..Default::default()
        });
        throttle.set_crawl_delay("www.example.com", Some(Duration::from_secs(30)));

        assert_eq!(
            throttle.crawl_delay_remaining("example.com"),
            Duration::ZERO
        );
        throttle.record_request_start("example.com");
        tokio::time::advance(Duration::from_secs(10)).await;

        assert_eq!(
            throttle.crawl_delay_remaining("www.example.com"),
            Duration::from_secs(20)
        );
        throttle.set_crawl_delay("example.com", None);
        assert_eq!(
            throttle.crawl_delay_remaining("example.com"),
            Duration::ZERO
        );
    }

    #[test]
    fn auto_throttle_uses_canonical_www_domain_identity() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            min_delay: Duration::ZERO,
            ..Default::default()
        });

        throttle.record_success_latency("www.example.com", Duration::from_millis(400));

        assert_eq!(
            throttle.latency_ms("example.com"),
            throttle.latency_ms("www.example.com")
        );
    }
}
