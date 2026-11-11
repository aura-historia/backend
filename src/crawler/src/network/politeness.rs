//! Politeness groups: the hosts whose requests share one request budget and
//! one rate-limit backoff ([politeness groups](../../../../docs/crawler/networking.md#politeness-groups)).
//!
//! A group spaces request starts across all its hosts by its current spacing,
//! never below the group's minimum. A 429 anywhere in the group pauses every
//! host of the group for the answer's `Retry-After` and doubles the spacing;
//! each successful response eases the spacing back towards the minimum. One
//! [`PolitenessGroups`] is shared by every component of the process that
//! requests shop pages, so they all count against the same groups.

use crate::network::policy::canonical_crawl_host;
use dashmap::DashMap;
use dashmap::mapref::one::RefMut;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::Instant;

/// Multi-tenant shop platforms whose shops, each on its own subdomain, sit
/// behind one rate limit, with the minimum spacing between request starts the
/// whole platform gets.
const SHARED_RATE_LIMIT_PLATFORMS: &[(&str, Duration)] =
    &[("shopnetzwerk.com", Duration::from_millis(250))];

const MAX_ENTRIES: usize = 10_000;
/// The group pause after a 429 without a usable `Retry-After`.
const RATE_LIMIT_DEFAULT_PAUSE: Duration = Duration::from_secs(10);
/// The longest group pause a 429 sets. A longer `Retry-After` is left to the
/// answering ListingSource's scrape circuit; the rest of the group resumes
/// after this pause at the raised spacing.
pub const RATE_LIMIT_MAX_PAUSE: Duration = Duration::from_secs(60);
/// The group spacing a first 429 raises to.
const RATE_LIMIT_SPACING_STEP: Duration = Duration::from_secs(1);
const RATE_LIMIT_MAX_SPACING: Duration = Duration::from_secs(10);
/// The share of the raised spacing above the group minimum that each
/// successful response keeps.
const RATE_LIMIT_SPACING_RECOVERY: f64 = 0.95;
/// A raised spacing this close to the group minimum is back at the minimum.
const RATE_LIMIT_SPACING_SNAP: Duration = Duration::from_millis(10);

/// The hosts whose requests share one request budget and one rate-limit
/// backoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolitenessGroup {
    /// The platform domain for a shop on a listed shared platform, otherwise
    /// the host's registrable domain, or the canonical host when it has none.
    pub key: String,
    /// The spacing between request starts the group never drops below.
    pub min_spacing: Duration,
}

/// The politeness group of `host`: its shared platform when it is one of a
/// listed platform's subdomains, otherwise its registrable domain.
pub fn politeness_group(host: &str) -> PolitenessGroup {
    let host = canonical_crawl_host(host);
    if let Some((platform, min_spacing)) =
        SHARED_RATE_LIMIT_PLATFORMS.iter().find(|(platform, _)| {
            host.strip_suffix(platform)
                .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('.'))
        })
    {
        return PolitenessGroup {
            key: (*platform).to_owned(),
            min_spacing: *min_spacing,
        };
    }
    PolitenessGroup {
        key: registrable_domain(&host).unwrap_or(host),
        min_spacing: Duration::ZERO,
    }
}

/// The registrable domain of `host` (one label below its public suffix) by
/// the public suffix list. The list's private section counts, so the tenants
/// of a hosting platform listed there (`shop.myshopify.com`) stay apart.
/// `None` for an IP address or a host that is itself a public suffix.
fn registrable_domain(host: &str) -> Option<String> {
    if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return None;
    }
    psl::domain_str(host).map(str::to_owned)
}

/// How a 429 slowed its politeness group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitBackoff {
    /// How long every host of the group waits before its next request.
    pub pause: Duration,
    /// The group's spacing between request starts after the 429.
    pub spacing: Duration,
}

/// A politeness group's shared pacing across its hosts.
struct GroupGate {
    min_spacing: Duration,
    /// The spacing between request starts; at least `min_spacing`.
    spacing: Duration,
    paused_until: Option<Instant>,
    /// The latest reserved request start of any host in the group.
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

/// A latest start a reservation took over, with the one it replaced.
pub(crate) struct StartSlot {
    key: String,
    start: Instant,
    previous: Option<Instant>,
}

impl StartSlot {
    /// Takes over `latest` when `start` is later than it.
    pub(crate) fn book(key: &str, latest: &mut Option<Instant>, start: Instant) -> Option<Self> {
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

    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    /// Hands the start back, unless a later reservation already queued behind it.
    pub(crate) fn release(&self, latest: &mut Option<Instant>) {
        if *latest == Some(self.start) {
            *latest = self.previous;
        }
    }
}

/// A request start reserved in a politeness group. Waiting until
/// [`GroupStart::start`] and then calling [`GroupStart::claim`] uses it;
/// dropping it unclaimed hands it back.
#[must_use]
pub struct GroupStart<'a> {
    groups: &'a PolitenessGroups,
    start: Instant,
    slot: Option<StartSlot>,
}

impl GroupStart<'_> {
    /// When the request may start.
    pub fn start(&self) -> Instant {
        self.start
    }

    /// Marks the start as used: the request is being sent.
    pub fn claim(mut self) {
        self.slot = None;
    }
}

impl Drop for GroupStart<'_> {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take()
            && let Some(mut gate) = self.groups.gates.get_mut(slot.key())
        {
            slot.release(&mut gate.last_request_start);
        }
    }
}

/// The process's politeness groups, shared by every component that requests
/// shop pages.
#[derive(Default)]
pub struct PolitenessGroups {
    gates: DashMap<String, GroupGate>,
    access_counter: AtomicU64,
}

impl PolitenessGroups {
    /// Reserves the next request start of `host`'s group, no earlier than
    /// `not_before`.
    pub fn reserve_start(&self, host: &str, not_before: Instant) -> GroupStart<'_> {
        let Some(mut gate) = self.gate(host) else {
            return GroupStart {
                groups: self,
                start: not_before,
                slot: None,
            };
        };
        let start = gate
            .earliest_start()
            .map_or(not_before, |earliest| earliest.max(not_before));
        let key = gate.key().clone();
        let slot = StartSlot::book(&key, &mut gate.last_request_start, start);
        GroupStart {
            groups: self,
            start,
            slot,
        }
    }

    /// Records a successful response from `host`: one step of its group's
    /// recovery from a rate limit.
    pub fn record_response(&self, host: &str) {
        if let Some(mut gate) = self.gate(host) {
            gate.responded();
        }
    }

    /// Records a 429 from `host`: its whole group pauses for the answer's
    /// `Retry-After` (bounded) and spaces its requests further apart.
    pub fn record_rate_limit(
        &self,
        host: &str,
        retry_after: Option<Duration>,
    ) -> Option<RateLimitBackoff> {
        let now = Instant::now();
        self.gate(host)
            .map(|mut gate| gate.rate_limited(now, retry_after))
    }

    /// The gate of `host`'s group, created on first use and marked as accessed.
    fn gate(&self, host: &str) -> Option<RefMut<'_, String, GroupGate>> {
        let PolitenessGroup { key, min_spacing } = politeness_group(host);
        if key.is_empty() {
            return None;
        }
        let access_counter = self.access_counter.fetch_add(1, Ordering::Relaxed);
        if !self.gates.contains_key(&key) {
            evict_oldest(&self.gates, |gate| &gate.last_access);
        }
        let gate = self
            .gates
            .entry(key)
            .or_insert_with(|| GroupGate::new(min_spacing, access_counter));
        gate.last_access.store(access_counter, Ordering::Relaxed);
        Some(gate)
    }

    #[cfg(test)]
    fn spacing(&self, host: &str) -> Option<Duration> {
        self.gates
            .get(&politeness_group(host).key)
            .map(|gate| gate.spacing)
    }
}

/// Removes the least recently accessed entry once the map is full.
pub(crate) fn evict_oldest<V>(map: &DashMap<String, V>, last_access: impl Fn(&V) -> &AtomicU64) {
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
    fn should_group_shared_platform_shops_under_the_platform_domain() {
        let shop = politeness_group("Paulvankuik.Shopnetzwerk.com.");
        let other_shop = politeness_group("www.antiquariat-zorn.shopnetzwerk.com");

        assert_eq!(shop.key, "shopnetzwerk.com");
        assert_eq!(other_shop, shop);
        assert!(shop.min_spacing > Duration::ZERO);
    }

    #[test]
    fn should_keep_unrelated_hosts_in_their_own_group() {
        let lookalike = politeness_group("notshopnetzwerk.com");

        assert_eq!(lookalike.key, "notshopnetzwerk.com");
        assert_eq!(lookalike.min_spacing, Duration::ZERO);
        assert_eq!(politeness_group("www.example.com").key, "example.com");
        assert_ne!(
            politeness_group("antiques.example.co.uk").key,
            politeness_group("antiques.example2.co.uk").key
        );
    }

    #[test]
    fn should_group_hosts_by_registrable_domain_when_no_platform_is_listed() {
        assert_eq!(politeness_group("catalog.example.com").key, "example.com");
        assert_eq!(
            politeness_group("shop.antiques.co.uk").key,
            "antiques.co.uk"
        );
        assert_eq!(
            politeness_group("Catalog.Example.CO.UK.").min_spacing,
            Duration::ZERO
        );
    }

    #[test]
    fn should_keep_tenants_of_a_listed_hosting_suffix_apart() {
        assert_eq!(
            politeness_group("vintage-finds.myshopify.com").key,
            "vintage-finds.myshopify.com"
        );
        assert_ne!(
            politeness_group("a.github.io").key,
            politeness_group("b.github.io").key
        );
    }

    #[test]
    fn should_keep_hosts_without_a_registrable_domain_as_they_are() {
        assert_eq!(politeness_group("127.0.0.1").key, "127.0.0.1");
        assert_eq!(politeness_group("localhost").key, "localhost");
        assert_eq!(politeness_group("co.uk").key, "co.uk");
    }

    fn wait(groups: &PolitenessGroups, host: &str) -> Duration {
        let now = Instant::now();
        let start = groups.reserve_start(host, now);
        let wait = start.start() - now;
        start.claim();
        wait
    }

    #[tokio::test(start_paused = true)]
    async fn should_space_request_starts_across_shops_when_they_share_a_platform() {
        let groups = PolitenessGroups::default();
        let min_spacing = politeness_group("a.shopnetzwerk.com").min_spacing;

        let first = wait(&groups, "a.shopnetzwerk.com");
        let second = wait(&groups, "b.shopnetzwerk.com");
        let unrelated = wait(&groups, "example.com");

        assert_eq!(first, Duration::ZERO);
        assert_eq!(second, min_spacing);
        assert_eq!(unrelated, Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn should_pause_the_whole_group_when_one_shop_answers_429() {
        let groups = PolitenessGroups::default();

        let backoff = groups
            .record_rate_limit("a.shopnetzwerk.com", Some(Duration::from_secs(8)))
            .unwrap();
        let other_shop = wait(&groups, "b.shopnetzwerk.com");
        let following_shop = wait(&groups, "c.shopnetzwerk.com");
        let unrelated = wait(&groups, "example.com");

        assert_eq!(backoff.pause, Duration::from_secs(8));
        assert_eq!(backoff.spacing, RATE_LIMIT_SPACING_STEP);
        assert_eq!(other_shop, Duration::from_secs(8));
        assert_eq!(
            following_shop,
            Duration::from_secs(8) + RATE_LIMIT_SPACING_STEP
        );
        assert_eq!(unrelated, Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn should_bound_the_group_pause_when_retry_after_is_long() {
        let groups = PolitenessGroups::default();

        let backoff = groups
            .record_rate_limit("a.shopnetzwerk.com", Some(Duration::from_secs(600)))
            .unwrap();

        assert_eq!(backoff.pause, RATE_LIMIT_MAX_PAUSE);
        assert_eq!(wait(&groups, "b.shopnetzwerk.com"), RATE_LIMIT_MAX_PAUSE);
    }

    #[tokio::test(start_paused = true)]
    async fn should_raise_the_spacing_on_repeated_429s_and_recover_it_on_responses() {
        let groups = PolitenessGroups::default();
        let group = politeness_group("a.shopnetzwerk.com");

        groups.record_rate_limit("a.shopnetzwerk.com", None);
        let raised = groups
            .record_rate_limit("b.shopnetzwerk.com", None)
            .unwrap()
            .spacing;
        for _ in 0..200 {
            groups.record_response("c.shopnetzwerk.com");
        }

        assert_eq!(raised, RATE_LIMIT_SPACING_STEP * 2);
        assert_eq!(
            groups.spacing("a.shopnetzwerk.com"),
            Some(group.min_spacing)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_an_unclaimed_group_start_hands_it_back() {
        let groups = PolitenessGroups::default();
        let min_spacing = politeness_group("a.shopnetzwerk.com").min_spacing;

        wait(&groups, "a.shopnetzwerk.com");
        drop(groups.reserve_start("b.shopnetzwerk.com", Instant::now()));

        assert_eq!(wait(&groups, "c.shopnetzwerk.com"), min_spacing);
    }
}
