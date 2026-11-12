//! Per-domain scraper pacing. Two rules share one per-domain entry:
//!
//! - the latency throttle: a delay before each request, derived from the
//!   domain's response latency and clamped to the configured minimum and
//!   maximum;
//! - the robots.txt `Crawl-delay`: a minimum spacing between request starts.
//!   Each request reserves its start under the domain's lock, so concurrent
//!   callers (scraper, schema seeds, review live inspection) queue behind one
//!   another instead of starting together. The spacing is measured from the
//!   previous start rather than slept in full, so a domain's first request is
//!   not held back. A reservation dropped before its request starts (a
//!   caller timing out while it waits) hands its start back.
//!
//! Above the domain sits its politeness group ([`crate::network::politeness`]),
//! which every reservation also books a start in.

use crate::network::policy::canonical_crawl_host;
use crate::network::politeness::{
    GroupStart, PolitenessGroups, RateLimitBackoff, StartSlot, evict_oldest,
};
use dashmap::DashMap;
use dashmap::mapref::one::RefMut;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::Instant;

const DEFAULT_TARGET_CONCURRENCY: f64 = 2.0;
const DEFAULT_ALPHA: f64 = 0.15;
const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(10);

/// A domain's smoothed response latency and the number of responses behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DomainLatencyStats {
    pub smoothed: Duration,
    pub samples: u64,
}

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
    /// The latest reserved request start.
    last_request_start: Option<Instant>,
}

impl DomainLatency {
    fn new(access_counter: u64) -> Self {
        Self {
            ema_us: AtomicU64::new(0),
            samples: AtomicU64::new(0),
            last_access: AtomicU64::new(access_counter),
            last_request_start: None,
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

/// A reserved request start. Waiting for it and then calling
/// [`RequestStart::claim`] uses it; dropping it unclaimed hands it back.
#[must_use]
pub struct RequestStart<'a> {
    throttle: &'a ScraperAutoThrottle,
    wait: Duration,
    /// Set while this reservation holds the domain's latest start.
    domain: Option<StartSlot>,
    /// The start this reservation holds in the domain's politeness group.
    group: Option<GroupStart<'a>>,
}

impl<'a> RequestStart<'a> {
    fn unbooked(throttle: &'a ScraperAutoThrottle, wait: Duration) -> Self {
        Self {
            throttle,
            wait,
            domain: None,
            group: None,
        }
    }

    /// How long to wait before the request starts.
    pub fn wait(&self) -> Duration {
        self.wait
    }

    /// Marks the start as used: the request is being sent.
    pub fn claim(mut self) {
        self.domain = None;
        if let Some(group) = self.group.take() {
            group.claim();
        }
    }
}

impl Drop for RequestStart<'_> {
    fn drop(&mut self) {
        if let Some(slot) = self.domain.take()
            && let Some(mut entry) = self.throttle.domains.get_mut(slot.key())
        {
            slot.release(&mut entry.last_request_start);
        }
    }
}

pub struct ScraperAutoThrottle {
    domains: DashMap<String, DomainLatency>,
    groups: Arc<PolitenessGroups>,
    config: ScraperAutoThrottleConfig,
    access_counter: AtomicU64,
}

impl ScraperAutoThrottle {
    pub fn new(config: ScraperAutoThrottleConfig) -> Self {
        Self {
            domains: DashMap::with_capacity(64),
            groups: Arc::default(),
            config,
            access_counter: AtomicU64::new(0),
        }
    }

    /// The politeness groups this throttle books its starts in, for the other
    /// components of the process to share.
    pub fn politeness_groups(&self) -> &Arc<PolitenessGroups> {
        &self.groups
    }

    pub fn record_latency(&self, domain: &str, latency: Duration) {
        self.record_success_latency(domain, latency);
    }

    /// Records a successful response: its latency, and one step of the
    /// group's recovery from a rate limit.
    pub fn record_success_latency(&self, domain: &str, latency: Duration) {
        self.record_with(domain, latency, false);
        self.groups.record_response(domain);
    }

    /// Records a 429 from `domain`: its whole politeness group pauses for the
    /// answer's `Retry-After` (bounded) and spaces its requests further apart.
    pub fn record_rate_limit(
        &self,
        domain: &str,
        retry_after: Option<Duration>,
    ) -> Option<RateLimitBackoff> {
        self.groups.record_rate_limit(domain, retry_after)
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
        let domain = canonical_crawl_host(raw_domain);
        if domain.is_empty() {
            return None;
        }
        let access_counter = self.access_counter.fetch_add(1, Ordering::Relaxed);
        if !self.domains.contains_key(&domain) {
            evict_oldest(&self.domains, |entry| &entry.last_access);
        }
        let entry = self
            .domains
            .entry(domain)
            .or_insert_with(|| DomainLatency::new(access_counter));
        entry.last_access.store(access_counter, Ordering::Relaxed);
        Some(entry)
    }

    /// Reserves the domain's next request start: at least `min_wait` from
    /// now, at least `crawl_delay` after the domain's previously reserved
    /// start, and no earlier than its politeness group admits. Applies even
    /// when the latency throttle is disabled: robots.txt and a shop's rate
    /// limit are not optional.
    pub fn reserve_request_start(
        &self,
        domain: &str,
        min_wait: Duration,
        crawl_delay: Option<Duration>,
    ) -> RequestStart<'_> {
        let now = Instant::now();
        let Some(mut entry) = self.entry(domain) else {
            return RequestStart::unbooked(self, min_wait);
        };
        let mut not_before = now + min_wait;
        if let (Some(delay), Some(previous)) = (crawl_delay, entry.last_request_start) {
            not_before = not_before.max(previous + delay);
        }
        // Booked while the domain's entry is held, so the domain's starts and
        // its group's stay in the same order.
        let group = self.groups.reserve_start(domain, not_before);
        let start = group.start();
        let domain_key = entry.key().clone();
        RequestStart {
            throttle: self,
            wait: start - now,
            domain: StartSlot::book(&domain_key, &mut entry.last_request_start, start),
            group: Some(group),
        }
    }

    pub fn delay_for(&self, domain: &str) -> Duration {
        if !self.config.enabled {
            return Duration::ZERO;
        }

        let domain = canonical_crawl_host(domain);
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
        self.latency(domain)
            .map(|latency| latency.smoothed.as_micros() as f64 / 1000.0)
    }

    /// The domain's smoothed response latency, `None` before its first response.
    pub fn latency(&self, domain: &str) -> Option<DomainLatencyStats> {
        let domain = canonical_crawl_host(domain);
        self.domains.get(&domain).and_then(|entry| {
            let samples = entry.samples.load(Ordering::Relaxed);
            (samples > 0).then(|| DomainLatencyStats {
                smoothed: Duration::from_micros(entry.ema_micros().max(0.0) as u64),
                samples,
            })
        })
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
    async fn crawl_delay_queues_request_starts_across_bare_and_www_hosts() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            enabled: false,
            ..Default::default()
        });
        let crawl_delay = Some(Duration::from_secs(30));

        let first = throttle.reserve_request_start("example.com", Duration::ZERO, crawl_delay);
        let second = throttle.reserve_request_start("www.example.com", Duration::ZERO, crawl_delay);
        tokio::time::advance(Duration::from_secs(40)).await;
        let third = throttle.reserve_request_start("example.com", Duration::ZERO, crawl_delay);

        assert_eq!(first.wait(), Duration::ZERO);
        // Reserved while the first was still pending: it queues behind it.
        assert_eq!(second.wait(), Duration::from_secs(30));
        assert_eq!(third.wait(), Duration::from_secs(20));
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_an_unclaimed_reservation_hands_its_start_back() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            enabled: false,
            ..Default::default()
        });
        let crawl_delay = Some(Duration::from_secs(30));

        throttle
            .reserve_request_start("example.com", Duration::ZERO, crawl_delay)
            .claim();
        drop(throttle.reserve_request_start("example.com", Duration::ZERO, crawl_delay));
        let next = throttle.reserve_request_start("example.com", Duration::ZERO, crawl_delay);

        // Spaced from the claimed start only, not from the abandoned one.
        assert_eq!(next.wait(), Duration::from_secs(30));
    }

    #[test]
    fn reservation_without_crawl_delay_waits_only_the_minimum() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig::default());

        throttle
            .reserve_request_start("example.com", Duration::ZERO, Some(Duration::from_secs(30)))
            .claim();

        assert_eq!(
            throttle
                .reserve_request_start("example.com", Duration::from_millis(5), None)
                .wait(),
            Duration::from_millis(5)
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

    #[tokio::test(start_paused = true)]
    async fn should_hold_a_domain_start_behind_its_group_pause() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            enabled: false,
            ..Default::default()
        });

        throttle.record_rate_limit("a.shopnetzwerk.com", Some(Duration::from_secs(8)));
        let other_shop = throttle.reserve_request_start("b.shopnetzwerk.com", Duration::ZERO, None);
        let unrelated = throttle.reserve_request_start("example.com", Duration::ZERO, None);

        assert_eq!(other_shop.wait(), Duration::from_secs(8));
        assert_eq!(unrelated.wait(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn should_share_group_starts_with_other_components_of_the_process() {
        let throttle = ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            enabled: false,
            ..Default::default()
        });
        let shared = Arc::clone(throttle.politeness_groups());

        shared
            .reserve_start("a.shopnetzwerk.com", Instant::now())
            .claim();
        let scraper_start =
            throttle.reserve_request_start("b.shopnetzwerk.com", Duration::ZERO, None);

        assert!(scraper_start.wait() > Duration::ZERO);
    }
}
