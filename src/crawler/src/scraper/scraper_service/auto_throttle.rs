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
//! Above the domain sits its politeness group (`network::policy::politeness_group`):
//! the shops of one shared platform, or the domain alone. The group spaces
//! request starts across all its domains by its current spacing, never below
//! the group's minimum. A 429 anywhere in the group pauses every domain of the
//! group for the answer's `Retry-After` and doubles the spacing; each
//! successful response eases the spacing back towards the minimum.

use crate::network::policy::{PolitenessGroup, canonical_crawl_host, politeness_group};
use dashmap::DashMap;
use dashmap::mapref::one::RefMut;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::Instant;

const DEFAULT_TARGET_CONCURRENCY: f64 = 2.0;
const DEFAULT_ALPHA: f64 = 0.15;
const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(10);
const MAX_ENTRIES: usize = 10_000;
/// The group pause after a 429 without a usable `Retry-After`.
const RATE_LIMIT_DEFAULT_PAUSE: Duration = Duration::from_secs(10);
/// The longest group pause a 429 sets. A longer `Retry-After` is left to the
/// answering ListingSource's scrape circuit; the rest of the group resumes
/// after this pause at the raised spacing.
const RATE_LIMIT_MAX_PAUSE: Duration = Duration::from_secs(60);
/// The group spacing a first 429 raises to.
const RATE_LIMIT_SPACING_STEP: Duration = Duration::from_secs(1);
const RATE_LIMIT_MAX_SPACING: Duration = Duration::from_secs(10);
/// The share of the raised spacing above the group minimum that each
/// successful response keeps.
const RATE_LIMIT_SPACING_RECOVERY: f64 = 0.95;
/// A raised spacing this close to the group minimum is back at the minimum.
const RATE_LIMIT_SPACING_SNAP: Duration = Duration::from_millis(10);

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

/// A politeness group's shared pacing across its domains.
struct GroupGate {
    min_spacing: Duration,
    /// The spacing between request starts; at least `min_spacing`.
    spacing: Duration,
    paused_until: Option<Instant>,
    /// The latest reserved request start of any domain in the group.
    last_request_start: Option<Instant>,
    last_access: AtomicU64,
}

impl GroupGate {
    fn new(min_spacing: Duration, access_counter: u64) -> Self {
        Self {
            min_spacing,
            spacing: min_spacing,
            paused_until: None,
            last_request_start: None,
            last_access: AtomicU64::new(access_counter),
        }
    }

    /// The earliest start the group admits: after its pause, and the spacing
    /// after its latest reserved start.
    fn earliest_start(&self) -> Option<Instant> {
        let spaced = self
            .last_request_start
            .filter(|_| !self.spacing.is_zero())
            .map(|previous| previous + self.spacing);
        spaced.max(self.paused_until)
    }

    fn rate_limited(&mut self, now: Instant, retry_after: Option<Duration>) -> RateLimitBackoff {
        let pause = retry_after
            .unwrap_or(RATE_LIMIT_DEFAULT_PAUSE)
            .min(RATE_LIMIT_MAX_PAUSE);
        self.paused_until = self.paused_until.max(Some(now + pause));
        self.spacing = (self.spacing * 2)
            .max(RATE_LIMIT_SPACING_STEP)
            .min(RATE_LIMIT_MAX_SPACING)
            .max(self.min_spacing);
        RateLimitBackoff {
            pause,
            spacing: self.spacing,
        }
    }

    fn responded(&mut self) {
        let raised = self.spacing.saturating_sub(self.min_spacing);
        let kept = raised.mul_f64(RATE_LIMIT_SPACING_RECOVERY);
        self.spacing = if kept < RATE_LIMIT_SPACING_SNAP {
            self.min_spacing
        } else {
            self.min_spacing + kept
        };
    }
}

/// How a 429 slowed its politeness group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitBackoff {
    /// How long every domain of the group waits before its next request.
    pub pause: Duration,
    /// The group's spacing between request starts after the 429.
    pub spacing: Duration,
}

/// A reserved request start. Waiting for it and then calling
/// [`RequestStart::claim`] uses it; dropping it unclaimed hands it back.
#[must_use]
pub struct RequestStart<'a> {
    throttle: &'a ScraperAutoThrottle,
    wait: Duration,
    /// The domain's and the group's latest starts this reservation holds.
    booking: Booking,
}

#[derive(Default)]
struct Booking {
    domain: Option<Slot>,
    group: Option<Slot>,
}

/// A latest start a reservation took over, with the one it replaced.
struct Slot {
    key: String,
    start: Instant,
    previous: Option<Instant>,
}

impl Slot {
    /// Takes over `latest` when `start` is later than it.
    fn book(key: &str, latest: &mut Option<Instant>, start: Instant) -> Option<Self> {
        let previous = *latest;
        previous.is_none_or(|previous| start > previous).then(|| {
            *latest = Some(start);
            Self {
                key: key.to_owned(),
                start,
                previous,
            }
        })
    }

    /// Hands the start back, unless a later reservation already queued behind it.
    fn release(&self, latest: &mut Option<Instant>) {
        if *latest == Some(self.start) {
            *latest = self.previous;
        }
    }
}

impl<'a> RequestStart<'a> {
    fn unbooked(throttle: &'a ScraperAutoThrottle, wait: Duration) -> Self {
        Self {
            throttle,
            wait,
            booking: Booking::default(),
        }
    }

    /// How long to wait before the request starts.
    pub fn wait(&self) -> Duration {
        self.wait
    }

    /// Marks the start as used: the request is being sent.
    pub fn claim(mut self) {
        self.booking = Booking::default();
    }
}

impl Drop for RequestStart<'_> {
    fn drop(&mut self) {
        let booking = std::mem::take(&mut self.booking);
        self.throttle.release(&booking);
    }
}

pub struct ScraperAutoThrottle {
    domains: DashMap<String, DomainLatency>,
    groups: DashMap<String, GroupGate>,
    config: ScraperAutoThrottleConfig,
    access_counter: AtomicU64,
}

impl ScraperAutoThrottle {
    pub fn new(config: ScraperAutoThrottleConfig) -> Self {
        Self {
            domains: DashMap::with_capacity(64),
            groups: DashMap::with_capacity(64),
            config,
            access_counter: AtomicU64::new(0),
        }
    }

    pub fn record_latency(&self, domain: &str, latency: Duration) {
        self.record_success_latency(domain, latency);
    }

    /// Records a successful response: its latency, and one step of the
    /// group's recovery from a rate limit.
    pub fn record_success_latency(&self, domain: &str, latency: Duration) {
        self.record_with(domain, latency, false);
        if let Some(mut gate) = self.group_entry(domain) {
            gate.responded();
        }
    }

    /// Records a 429 from `domain`: its whole politeness group pauses for the
    /// answer's `Retry-After` (bounded) and spaces its requests further apart.
    pub fn record_rate_limit(
        &self,
        domain: &str,
        retry_after: Option<Duration>,
    ) -> Option<RateLimitBackoff> {
        let now = Instant::now();
        self.group_entry(domain)
            .map(|mut gate| gate.rate_limited(now, retry_after))
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

    /// The gate of the domain's politeness group, created on first use and
    /// marked as accessed. Taken after the domain's entry, never before it.
    fn group_entry(&self, raw_domain: &str) -> Option<RefMut<'_, String, GroupGate>> {
        let PolitenessGroup { key, min_spacing } = politeness_group(raw_domain);
        if key.is_empty() {
            return None;
        }
        let access_counter = self.access_counter.fetch_add(1, Ordering::Relaxed);
        if !self.groups.contains_key(&key) {
            evict_oldest(&self.groups, |gate| &gate.last_access);
        }
        let gate = self
            .groups
            .entry(key)
            .or_insert_with(|| GroupGate::new(min_spacing, access_counter));
        gate.last_access.store(access_counter, Ordering::Relaxed);
        Some(gate)
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
        let mut start = now + min_wait;
        if let (Some(delay), Some(previous)) = (crawl_delay, entry.last_request_start) {
            start = start.max(previous + delay);
        }
        let mut gate = self.group_entry(domain);
        if let Some(earliest) = gate.as_ref().and_then(|gate| gate.earliest_start()) {
            start = start.max(earliest);
        }
        let group = gate.as_mut().and_then(|gate| {
            let key = gate.key().clone();
            Slot::book(&key, &mut gate.last_request_start, start)
        });
        let domain_key = entry.key().clone();
        let booking = Booking {
            domain: Slot::book(&domain_key, &mut entry.last_request_start, start),
            group,
        };
        RequestStart {
            throttle: self,
            wait: start - now,
            booking,
        }
    }

    /// Hands back the starts of an unclaimed reservation.
    fn release(&self, booking: &Booking) {
        if let Some(slot) = &booking.domain
            && let Some(mut entry) = self.domains.get_mut(&slot.key)
        {
            slot.release(&mut entry.last_request_start);
        }
        if let Some(slot) = &booking.group
            && let Some(mut gate) = self.groups.get_mut(&slot.key)
        {
            slot.release(&mut gate.last_request_start);
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

/// Removes the least recently accessed entry once the map is full.
fn evict_oldest<V>(map: &DashMap<String, V>, last_access: impl Fn(&V) -> &AtomicU64) {
    if map.len() < MAX_ENTRIES {
        return;
    }

    let mut oldest_key: Option<String> = None;
    let mut oldest_access = u64::MAX;
    for entry in map.iter() {
        let access = last_access(entry.value()).load(Ordering::Relaxed);
        if access < oldest_access {
            oldest_access = access;
            oldest_key = Some(entry.key().clone());
        }
    }

    if let Some(key) = oldest_key {
        map.remove(&key);
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

    fn pacing_only() -> ScraperAutoThrottle {
        ScraperAutoThrottle::new(ScraperAutoThrottleConfig {
            enabled: false,
            ..Default::default()
        })
    }

    #[tokio::test(start_paused = true)]
    async fn should_space_request_starts_across_shops_when_they_share_a_platform() {
        let throttle = pacing_only();
        let min_spacing = politeness_group("a.shopnetzwerk.com").min_spacing;

        let first = throttle.reserve_request_start("a.shopnetzwerk.com", Duration::ZERO, None);
        let second = throttle.reserve_request_start("b.shopnetzwerk.com", Duration::ZERO, None);
        let unrelated = throttle.reserve_request_start("example.com", Duration::ZERO, None);

        assert_eq!(first.wait(), Duration::ZERO);
        assert_eq!(second.wait(), min_spacing);
        assert_eq!(unrelated.wait(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn should_pause_the_whole_group_when_one_shop_answers_429() {
        let throttle = pacing_only();

        let backoff = throttle
            .record_rate_limit("a.shopnetzwerk.com", Some(Duration::from_secs(8)))
            .unwrap();
        let other_shop = throttle.reserve_request_start("b.shopnetzwerk.com", Duration::ZERO, None);
        let following_shop =
            throttle.reserve_request_start("c.shopnetzwerk.com", Duration::ZERO, None);
        let unrelated = throttle.reserve_request_start("example.com", Duration::ZERO, None);

        assert_eq!(backoff.pause, Duration::from_secs(8));
        assert_eq!(backoff.spacing, RATE_LIMIT_SPACING_STEP);
        assert_eq!(other_shop.wait(), Duration::from_secs(8));
        assert_eq!(
            following_shop.wait(),
            Duration::from_secs(8) + RATE_LIMIT_SPACING_STEP
        );
        assert_eq!(unrelated.wait(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn should_bound_the_group_pause_when_retry_after_is_long() {
        let throttle = pacing_only();

        let backoff = throttle
            .record_rate_limit("a.shopnetzwerk.com", Some(Duration::from_secs(600)))
            .unwrap();

        assert_eq!(backoff.pause, RATE_LIMIT_MAX_PAUSE);
        assert_eq!(
            throttle
                .reserve_request_start("b.shopnetzwerk.com", Duration::ZERO, None)
                .wait(),
            RATE_LIMIT_MAX_PAUSE
        );
    }

    #[tokio::test(start_paused = true)]
    async fn should_raise_the_spacing_on_repeated_429s_and_recover_it_on_responses() {
        let throttle = pacing_only();
        let group = politeness_group("a.shopnetzwerk.com");

        throttle.record_rate_limit("a.shopnetzwerk.com", None);
        let raised = throttle
            .record_rate_limit("b.shopnetzwerk.com", None)
            .unwrap()
            .spacing;
        for _ in 0..200 {
            throttle.record_success_latency("c.shopnetzwerk.com", Duration::from_millis(100));
        }

        assert_eq!(raised, RATE_LIMIT_SPACING_STEP * 2);
        assert_eq!(
            throttle.groups.get(&group.key).unwrap().spacing,
            group.min_spacing
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_an_unclaimed_reservation_hands_its_group_start_back() {
        let throttle = pacing_only();
        let min_spacing = politeness_group("a.shopnetzwerk.com").min_spacing;

        throttle
            .reserve_request_start("a.shopnetzwerk.com", Duration::ZERO, None)
            .claim();
        drop(throttle.reserve_request_start("b.shopnetzwerk.com", Duration::ZERO, None));
        let next = throttle.reserve_request_start("c.shopnetzwerk.com", Duration::ZERO, None);

        assert_eq!(next.wait(), min_spacing);
    }
}
