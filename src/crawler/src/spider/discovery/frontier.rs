//! What a crawl visits besides the links it finds from its root, and what it leaves unvisited
//! (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
//!
//! Link discovery from the root stays the authority on what a shop lists. Seeds, the URLs an
//! earlier cut-short crawl left unvisited, only extend it: they are fetched, filtered and stored
//! like found links, within the same crawl budget. A frontier larger than one batch of seeds is
//! kept, not dropped: the oldest URLs are seeded first, the rest in later batches while the crawl
//! has budget, and what it does not reach is carried over to a later crawl.

use regex::{Regex, RegexSet};
use serde::{Deserialize, Serialize};
use spider::CaseInsensitiveString;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use url::Url;

use super::known_pages::{KnownPage, KnownPages};
use super::page_base::link_base;
use super::robots::CrawlRobots;
use super::website_spider::{CrawlBudgetScale, CrawlPace};
use crate::spider::classification::product_url_pattern::ProductUrlPattern;
use crate::spider::utils::url::CrawledUrl;

/// Most URLs one batch of a crawl hands Spider as seeds. Spider keeps at most 25,000 seed links
/// in memory.
pub const MAX_SEED_URLS: usize = 20_000;

/// Most URLs a crawl leaves for the next ones, which bounds a ListingSource's stored frontier: a
/// crawl seeds the oldest [`MAX_SEED_URLS`] of them first and the rest in later batches.
pub const MAX_FRONTIER_URLS: usize = 100_000;

/// Most distinct links one crawl remembers having found, which bounds the frontier it can
/// compute.
const MAX_FOUND_LINKS: usize = 200_000;

/// Which crawl of its schedule a ListingSource runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CrawlKind {
    /// Discovers the whole shop: every page the crawl reaches is fetched, and its last full crawl
    /// is recorded when nothing is left unvisited.
    #[default]
    Full,
    /// Rediscovers new product URLs between full crawls: listing and category pages are fetched
    /// again, product pages the ListingSource already stores are not.
    Incremental,
}

impl CrawlKind {
    /// Stable label for logs; not persisted.
    pub const fn as_str(self) -> &'static str {
        match self {
            CrawlKind::Full => "full",
            CrawlKind::Incremental => "incremental",
        }
    }
}

impl std::fmt::Display for CrawlKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a crawl starts from besides its root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrawlSeeds {
    /// URLs the ListingSource's previous crawls found or were seeded with but did not visit, the
    /// longest-waiting first.
    pub frontier: Vec<Url>,
    /// URLs the crawl does not fetch: the product URLs an incremental crawl already knows.
    pub known: Vec<Url>,
    /// URLs the crawl does not fetch either, as the crawler retired them as duplicates of
    /// another; unlike `known`, never counted as product pages the crawl found linked.
    pub retired: Vec<Url>,
}

/// One crawl of a ListingSource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlRequest {
    pub crawl_root_url: String,
    pub pace: CrawlPace,
    pub budget: CrawlBudgetScale,
    pub seeds: CrawlSeeds,
    /// The ListingSource's product URL pattern: the pages its sitemaps list that the pattern
    /// takes are stored without a fetch, and the crawl does not fetch them
    /// (`docs/crawler/networking.md#sitemaps`).
    pub product_pattern: Option<ProductUrlPattern>,
}

impl CrawlRequest {
    /// A crawl from the root alone, as the first crawl of a ListingSource runs.
    pub fn from_root(crawl_root_url: impl Into<String>, pace: CrawlPace) -> Self {
        Self {
            crawl_root_url: crawl_root_url.into(),
            pace,
            budget: CrawlBudgetScale::STANDARD,
            seeds: CrawlSeeds::default(),
            product_pattern: None,
        }
    }
}

/// What an ended crawl left for the next one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrawlDiscovery {
    /// URLs the crawl carried over, found or was seeded with but did not visit, at most
    /// [`MAX_FRONTIER_URLS`]: the frontier it was given first, then what it found.
    pub frontier: Vec<Url>,
    /// Unvisited URLs beyond [`MAX_FRONTIER_URLS`] that the crawl found and could not keep.
    pub dropped: usize,
}

/// The frontier figures of one spider run, which its logs carry so an operator can tell from one
/// continuation to the next whether a large shop's frontier shrinks, and which the scheduler reads
/// to tell a shop too large for one crawl.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontierProgress {
    /// The stored frontier the crawl continued.
    pub carried: usize,
    /// Of `carried`, the URLs beyond the crawl's first batch of seeds, which it seeds in later
    /// batches only while it has time and pages left.
    pub deferred: usize,
    /// The URLs the crawl left unvisited for the next one; `None` when the crawl could not tell,
    /// in which case the previously stored frontier is kept.
    pub left: Option<usize>,
    /// Unvisited URLs beyond [`MAX_FRONTIER_URLS`] the crawl could not keep.
    pub dropped: usize,
}

impl FrontierProgress {
    /// The figures of a crawl that continued `seeded` URLs in its first batch of seeds and
    /// `deferred` beyond it, and left `discovery`, or could not tell its frontier.
    pub fn after(seeded: usize, deferred: usize, discovery: Option<&CrawlDiscovery>) -> Self {
        Self {
            carried: seeded + deferred,
            deferred,
            left: discovery.map(|discovery| discovery.frontier.len()),
            dropped: discovery.map_or(0, |discovery| discovery.dropped),
        }
    }

    /// Whether the crawl left URLs unvisited.
    pub fn has_urls_left(&self) -> bool {
        self.left.is_some_and(|left| left > 0)
    }
}

/// The links Spider follows, in the spelling it requests them: on the crawl root's host alone
/// (its host whitelist), under the root's scheme, in the spelling the crawler stores and refused
/// by neither the URL blacklist nor Spider's deny list. A link Spider would not request can never
/// be visited, so it must never be carried over.
pub(super) struct FollowRules {
    root: Url,
    host_whitelist: Regex,
    deny_list: Arc<RegexSet>,
}

impl FollowRules {
    pub(super) fn new(root: Url, host_whitelist: Regex, deny_list: Arc<RegexSet>) -> Self {
        Self {
            root,
            host_whitelist,
            deny_list,
        }
    }

    /// `url` as Spider requests it, when Spider follows it. Spider moves every link it finds
    /// to the root's scheme.
    pub(super) fn followed(&self, mut url: Url) -> Option<CrawledUrl> {
        if matches!(url.scheme(), "http" | "https") && url.scheme() != self.root.scheme() {
            url.set_scheme(self.root.scheme()).ok()?;
        }
        let stored = CrawledUrl::new(url);
        let spelling = stored.as_url().as_str();
        let followed = self.host_whitelist.is_match(spelling)
            && !stored.is_blacklisted()
            && !self.deny_list.is_match(spelling);
        followed.then_some(stored)
    }
}

/// The stored frontier a crawl continues, in the spelling Spider requests it, the longest-waiting
/// first.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct FrontierSeeds {
    /// The oldest URLs, the crawl's first batch of seeds.
    pub(super) seeded: Vec<String>,
    /// The URLs beyond the first batch, seeded in later batches while the crawl has budget and
    /// carried over to the next crawl when it does not reach them.
    pub(super) deferred: Vec<String>,
}

impl FrontierSeeds {
    /// `frontier`, longest-waiting first, seeding at most `seed_limit` of it.
    fn split(mut frontier: Vec<String>, seed_limit: usize) -> Self {
        let deferred = frontier.split_off(frontier.len().min(seed_limit));
        Self {
            seeded: frontier,
            deferred,
        }
    }

    fn carried(&self) -> impl Iterator<Item = &String> {
        self.seeded.iter().chain(&self.deferred)
    }
}

/// Seeds a crawl may hand Spider: links Spider [follows](FollowRules) that robots.txt allows.
pub(super) struct SeedAdmission<'a> {
    pub(super) rules: &'a FollowRules,
    pub(super) robots: &'a CrawlRobots,
}

impl SeedAdmission<'_> {
    pub(super) fn admit(&self, url: &Url) -> Option<CrawledUrl> {
        self.rules
            .followed(url.clone())
            .filter(|stored| self.robots.allows(stored.as_url()))
    }

    /// The admitted seeds, each once and never the root itself, at most `limit`.
    pub(super) fn admit_up_to<'u>(
        &self,
        urls: impl IntoIterator<Item = &'u Url>,
        limit: usize,
    ) -> Vec<String> {
        let mut seen = HashSet::new();
        seen.insert(self.rules.root.as_str().to_string());
        urls.into_iter()
            .filter_map(|url| self.admit(url))
            .map(|stored| stored.as_url().as_str().to_string())
            .filter(|stored| seen.insert(stored.clone()))
            .take(limit)
            .collect()
    }

    /// The admitted part of the stored `frontier`, in its order, split at `seed_limit`.
    pub(super) fn frontier_seeds(&self, frontier: &[Url], seed_limit: usize) -> FrontierSeeds {
        FrontierSeeds::split(self.admit_up_to(frontier, MAX_FRONTIER_URLS), seed_limit)
    }
}

/// The links on the pages a crawl delivered, in the spelling the crawler stores, so what the
/// crawl did not reach can carry over. Spider reports a page's links with the page; it hands its
/// link callback only the links it goes on to fetch.
#[derive(Clone)]
pub(super) struct FoundLinks {
    rules: Arc<FollowRules>,
    links: Arc<Mutex<HashSet<String>>>,
    /// The pages Spider delivered. Spider counts a link visited once it starts
    /// the fetch, also when its crawl ended before the page arrived.
    delivered: Arc<Mutex<HashSet<CaseInsensitiveString>>>,
    /// Set once the crawl turns to the sitemap seeds ([`Self::stop_recording`]).
    recording_stopped: Arc<AtomicBool>,
}

impl FoundLinks {
    /// Records the links Spider follows by `rules`.
    pub(super) fn new(rules: Arc<FollowRules>) -> Self {
        Self {
            rules,
            links: Arc::default(),
            delivered: Arc::default(),
            recording_stopped: Arc::default(),
        }
    }

    pub(super) fn record_delivered(&self, page_url: &str) {
        let mut delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if delivered.len() < MAX_FOUND_LINKS {
            delivered.insert(CaseInsensitiveString::from(page_url));
        }
    }

    /// The carried frontier and the found links whose page the crawl neither delivered nor
    /// `visited` counts, each once, at most `limit`: the next batch of seeds. A page arrives in
    /// the spelling the crawler stores, which differs from the link Spider counts visited when
    /// the link carried parameters that do not identify the page.
    pub(super) fn unvisited(
        &self,
        carried: &FrontierSeeds,
        visited: impl Fn(&CaseInsensitiveString) -> bool,
        limit: usize,
    ) -> Vec<String> {
        let found = self.snapshot();
        self.unvisited_among(carried.carried().chain(found.iter()), visited, limit)
    }

    /// The `candidates` whose page the crawl neither delivered nor `visited` counts, each once,
    /// at most `limit`.
    pub(super) fn unvisited_among<'a>(
        &self,
        candidates: impl IntoIterator<Item = &'a String>,
        visited: impl Fn(&CaseInsensitiveString) -> bool,
        limit: usize,
    ) -> Vec<String> {
        let delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        unvisited(
            candidates,
            |link| delivered.contains(link) || visited(link),
            limit,
        )
    }

    /// Stops recording links: what the crawl finds from now on, while it visits the sitemap
    /// seeds in its leftover time, is no frontier for the next crawl.
    pub(super) fn stop_recording(&self) {
        self.recording_stopped.store(true, Ordering::Relaxed);
    }

    /// The carried frontier and the found links whose page the crawl neither delivered nor
    /// already knew, at most [`MAX_FRONTIER_URLS`].
    pub(super) fn frontier(&self, carried: &FrontierSeeds, known: &KnownPages) -> CrawlDiscovery {
        self.frontier_within(carried, known, MAX_FRONTIER_URLS)
    }

    /// The frontier, at most `limit` URLs: what the crawl was given keeps its place ahead of what
    /// it found, so the URLs that waited longest are kept and the newest found links dropped.
    fn frontier_within(
        &self,
        carried: &FrontierSeeds,
        known: &KnownPages,
        limit: usize,
    ) -> CrawlDiscovery {
        let found = self.snapshot();
        let mut left = self.unvisited_among(
            carried.carried().chain(found.iter()),
            |link| Url::parse(link.inner()).is_ok_and(|url| known.page_of(&url).is_some()),
            usize::MAX,
        );
        let dropped = left.len().saturating_sub(limit);
        left.truncate(limit);
        CrawlDiscovery {
            frontier: left.iter().filter_map(|url| Url::parse(url).ok()).collect(),
            dropped,
        }
    }

    /// How many known product pages the delivered pages link that the crawl did not deliver,
    /// each page once whatever its trailing slash.
    pub(super) fn skipped_known(&self, known: &KnownPages) -> usize {
        let delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let is_delivered = |url: &str| delivered.contains(&CaseInsensitiveString::from(url));
        self.snapshot()
            .iter()
            .filter(|link| !is_delivered(link))
            .filter_map(|link| Url::parse(link).ok())
            .filter_map(|url| known.page_of(&url))
            .filter(|(page, kind)| *kind == KnownPage::Product && !is_delivered(page.as_str()))
            .map(|(page, _)| page)
            .collect::<HashSet<_>>()
            .len()
    }

    /// Records the links of the page fetched as `page_url` with body `html`, written as the page
    /// writes them: Spider reports them unresolved. Each is resolved where Spider resolves it,
    /// against the page's [link base](link_base), so a relative link on a page whose `<base>`
    /// names the shop's root is recorded as the page Spider requests.
    pub(super) fn record<'a>(
        &self,
        page_url: &str,
        html: &[u8],
        page_links: impl IntoIterator<Item = &'a CaseInsensitiveString>,
    ) {
        let Ok(page_url) = Url::parse(page_url) else {
            return;
        };
        let base = link_base(&page_url, html);
        let stored: Vec<String> = page_links
            .into_iter()
            .filter_map(|link| self.stored_spelling(&base, link.inner()))
            .collect();
        self.remember(stored);
    }

    /// The redirect target `resolved` as Spider requests it, when Spider follows it.
    pub(super) fn redirect_target(&self, resolved: Url) -> Option<CrawledUrl> {
        self.rules.followed(resolved)
    }

    /// Records a followed redirect target as a found link, so a crawl that ends before fetching
    /// it carries it over.
    pub(super) fn record_redirect_target(&self, target: &CrawledUrl) {
        self.remember([target.as_url().as_str().to_string()]);
    }

    fn remember(&self, stored: impl IntoIterator<Item = String>) {
        if self.recording_stopped.load(Ordering::Relaxed) {
            return;
        }
        let mut links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
        for link in stored {
            if links.len() >= MAX_FOUND_LINKS {
                break;
            }
            links.insert(link);
        }
    }

    pub(super) fn snapshot(&self) -> HashSet<String> {
        self.links
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The link, resolved against `base`, as Spider would request it (`FoundLinkDecoder`), when
    /// Spider follows it.
    fn stored_spelling(&self, base: &Url, link: &str) -> Option<String> {
        let decoded = html_escape::decode_html_entities(link);
        let resolved = base.join(decoded.trim()).ok()?;
        let stored = self.rules.followed(resolved)?;
        Some(stored.as_url().as_str().to_string())
    }
}

/// The candidates `is_visited` does not know, each once, at most `limit`. Spider compares links
/// case-insensitively, so this does too.
pub(super) fn unvisited<'a>(
    candidates: impl IntoIterator<Item = &'a String>,
    is_visited: impl Fn(&CaseInsensitiveString) -> bool,
    limit: usize,
) -> Vec<String> {
    let mut kept = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| !is_visited(&CaseInsensitiveString::from(candidate.as_str())))
        .filter(|candidate| kept.insert(candidate.to_ascii_lowercase()))
        .take(limit)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::robots::{DenyList, RobotsRules};
    use crate::spider::discovery::website_spider::configured_host_whitelist;

    fn url(path: &str) -> Url {
        Url::parse(&format!("https://shop.example{path}")).unwrap()
    }

    /// The rules Spider follows links by on a crawl of `https://shop.example/`.
    fn follow_rules(deny: &str) -> FollowRules {
        let root = url("/");
        let host_whitelist = Regex::new(&configured_host_whitelist(&root).unwrap()).unwrap();
        FollowRules::new(
            root,
            host_whitelist,
            Arc::new(RegexSet::new([deny]).unwrap()),
        )
    }

    fn robots(body: &str) -> CrawlRobots {
        let rules = RobotsRules::parse(body);
        CrawlRobots {
            deny_list: DenyList::default(),
            crawl_delay: None,
            rules: Some(rules),
            policy: "rules",
        }
    }

    #[test]
    fn should_admit_seeds_spider_follows_on_the_root_host_that_robots_allows() {
        let robots = robots("User-agent: *\nDisallow: /private\n");
        let rules = follow_rules(r"/denied");
        let admission = SeedAdmission {
            rules: &rules,
            robots: &robots,
        };
        let seeds = [
            url("/product/1?utm_source=newsletter"),
            url("/product/1"),
            Url::parse("http://shop.example/product/6").unwrap(),
            Url::parse("https://www.shop.example/product/2").unwrap(),
            Url::parse("https://other.example/product/3").unwrap(),
            url("/private/4"),
            url("/denied/5"),
            url("/cart"),
            url("/"),
        ];

        assert_eq!(
            admission.frontier_seeds(&seeds, MAX_SEED_URLS).seeded,
            [
                "https://shop.example/product/1",
                "https://shop.example/product/6"
            ]
        );
    }

    fn spellings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| url(path).to_string()).collect()
    }

    #[test]
    fn should_seed_the_longest_waiting_frontier_urls_and_defer_the_rest() {
        let frontier = spellings(&["/old/1", "/old/2", "/new/1"]);

        assert_eq!(
            FrontierSeeds::split(frontier.clone(), 2),
            FrontierSeeds {
                seeded: spellings(&["/old/1", "/old/2"]),
                deferred: spellings(&["/new/1"]),
            }
        );
        assert_eq!(
            FrontierSeeds::split(frontier.clone(), 5),
            FrontierSeeds {
                seeded: frontier,
                deferred: Vec::new(),
            }
        );
    }

    /// A frontier larger than one crawl's seeds is carried over whole, ahead of the links the
    /// crawl found; only what exceeds the cap is dropped, and counted.
    #[test]
    fn should_carry_deferred_urls_over_ahead_of_found_links_and_count_what_exceeds_the_cap() {
        let found = FoundLinks::new(Arc::new(follow_rules(r"/denied")));
        let links: Vec<CaseInsensitiveString> = ["/found/1", "/found/2", "/found/3", "/seed/2"]
            .into_iter()
            .map(CaseInsensitiveString::from)
            .collect();
        found.record("https://shop.example/", b"", &links);
        found.record_delivered("https://shop.example/seed/1");
        let carried = FrontierSeeds {
            seeded: spellings(&["/seed/1", "/seed/2"]),
            deferred: spellings(&["/deferred/1"]),
        };
        let known = KnownPages::new(&url("/"), &CrawlSeeds::default());

        let discovery = found.frontier_within(&carried, &known, 3);

        assert_eq!(
            discovery.frontier[..2],
            [url("/seed/2"), url("/deferred/1")],
            "{discovery:?}"
        );
        assert!(discovery.frontier[2].path().starts_with("/found/"));
        assert_eq!(discovery.dropped, 2, "{discovery:?}");
        let whole = found.frontier_within(&carried, &known, 10);
        assert_eq!(whole.frontier.len(), 5, "{whole:?}");
        assert_eq!(whole.dropped, 0);
    }

    #[test]
    fn should_leave_out_visited_links_in_any_letter_case_and_bound_the_rest() {
        let visited: HashSet<CaseInsensitiveString> =
            [CaseInsensitiveString::from("https://shop.example/A")]
                .into_iter()
                .collect();
        let candidates = [
            "https://shop.example/a".to_string(),
            "https://shop.example/b".to_string(),
            "https://shop.example/B".to_string(),
            "https://shop.example/c".to_string(),
        ];

        assert_eq!(
            unvisited(&candidates, |link| visited.contains(link), 10),
            ["https://shop.example/b", "https://shop.example/c"]
        );
        assert_eq!(
            unvisited(&candidates, |link| visited.contains(link), 1).len(),
            1
        );
    }

    #[test]
    fn should_record_page_links_spider_follows_in_the_spelling_it_requests() {
        let found = FoundLinks::new(Arc::new(follow_rules(r"/denied")));
        let links: Vec<CaseInsensitiveString> = [
            "item.php?id=1&amp;utm_source=x",
            "http://shop.example/product/5",
            "https://www.shop.example/product/2",
            "https://other.example/product/3",
            "https://shop.example/denied/4",
            "https://shop.example/cart",
            "mailto:shop@shop.example",
        ]
        .into_iter()
        .map(CaseInsensitiveString::from)
        .collect();

        found.record("https://shop.example/catalogue/", b"", &links);

        assert_eq!(
            found.snapshot(),
            [
                "https://shop.example/catalogue/item.php?id=1".to_string(),
                "https://shop.example/product/5".to_string()
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn should_count_each_known_product_page_linked_but_not_delivered_once() {
        let found = FoundLinks::new(Arc::new(follow_rules(r"/denied")));
        let links: Vec<CaseInsensitiveString> = [
            "/product/1",
            "/product/1/",
            "/product/2",
            "/product/3",
            "/category/1",
            "/old/1",
        ]
        .into_iter()
        .map(CaseInsensitiveString::from)
        .collect();
        found.record("https://shop.example/", b"", &links);
        found.record_delivered("https://shop.example/product/2");
        let known = KnownPages::new(
            &url("/"),
            &CrawlSeeds {
                known: vec![url("/product/1"), url("/product/2"), url("/product/9")],
                retired: vec![url("/old/1")],
                ..CrawlSeeds::default()
            },
        );

        assert_eq!(found.skipped_known(&known), 1);
    }

    /// marrisantiqueglass.com: a nested listing page whose `<base>` names the shop's root links
    /// its products relatively from there.
    const NESTED_LISTING: &str = "https://shop.example/en-GB/latest-items_pg_904/page_904";
    const ROOT_BASE: &str =
        r#"<html><head><base href="https://shop.example/"></head><body></body></html>"#;

    fn nested_listing_links() -> Vec<CaseInsensitiveString> {
        ["en-GB/glass/vase/prod_13036", "en-GB/glass/bowl/prod_13037"]
            .into_iter()
            .map(CaseInsensitiveString::from)
            .collect()
    }

    #[test]
    fn should_resolve_links_against_the_page_base_when_counting_and_carrying_them_over() {
        let found = FoundLinks::new(Arc::new(follow_rules(r"/denied")));
        found.record_delivered(NESTED_LISTING);
        found.record(
            NESTED_LISTING,
            ROOT_BASE.as_bytes(),
            &nested_listing_links(),
        );
        let known = KnownPages::new(
            &url("/"),
            &CrawlSeeds {
                known: vec![url("/en-GB/glass/vase/prod_13036")],
                ..CrawlSeeds::default()
            },
        );

        assert_eq!(found.skipped_known(&known), 1);
        assert_eq!(
            found.frontier(&FrontierSeeds::default(), &known).frontier,
            [url("/en-GB/glass/bowl/prod_13037")]
        );
    }

    #[test]
    fn should_resolve_the_links_of_a_page_without_a_base_against_the_page_url() {
        let found = FoundLinks::new(Arc::new(follow_rules(r"/denied")));
        found.record(
            NESTED_LISTING,
            b"<html><head><title>Latest</title></head><body></body></html>",
            &nested_listing_links(),
        );

        assert_eq!(
            found.snapshot(),
            [
                "https://shop.example/en-GB/latest-items_pg_904/en-GB/glass/vase/prod_13036"
                    .to_string(),
                "https://shop.example/en-GB/latest-items_pg_904/en-GB/glass/bowl/prod_13037"
                    .to_string(),
            ]
            .into_iter()
            .collect()
        );
    }
}
