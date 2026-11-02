//! What a crawl visits besides the links it finds from its root, and what it leaves unvisited
//! (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
//!
//! Link discovery from the root stays the authority on what a shop lists. Seeds, the URLs an
//! earlier cut-short crawl left unvisited, only extend it: they are fetched, filtered and stored
//! like found links, within the same crawl budget.

use regex::{Regex, RegexSet};
use spider::CaseInsensitiveString;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use url::Url;

use super::robots::CrawlRobots;
use super::website_spider::CrawlPace;
use crate::spider::utils::url::CrawledUrl;

/// Most URLs one crawl hands Spider as seeds, and most it leaves for the next crawl. Spider keeps
/// at most 25,000 seed links in memory.
pub const MAX_FRONTIER_URLS: usize = 20_000;

/// Most distinct links one crawl remembers having found, which bounds the frontier it can
/// compute.
const MAX_FOUND_LINKS: usize = 200_000;

/// Which crawl of its schedule a domain runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CrawlKind {
    /// Discovers the whole shop: every page the crawl reaches is fetched, and its last full crawl
    /// is recorded when nothing is left unvisited.
    #[default]
    Full,
    /// Rediscovers new product URLs between full crawls: listing and category pages are fetched
    /// again, product pages the domain already stores are not.
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
    /// URLs the domain's previous crawl found or was seeded with but did not visit.
    pub frontier: Vec<Url>,
    /// URLs the crawl does not fetch: the product URLs an incremental crawl already knows.
    pub known: Vec<Url>,
    /// URLs the crawl does not fetch either, as the crawler retired them as duplicates of
    /// another; unlike `known`, never counted as product pages the crawl found linked.
    pub retired: Vec<Url>,
}

/// One crawl of a domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlRequest {
    pub crawl_root_url: String,
    pub pace: CrawlPace,
    pub seeds: CrawlSeeds,
}

impl CrawlRequest {
    /// A crawl from the root alone, as the first crawl of a domain runs.
    pub fn from_root(crawl_root_url: impl Into<String>, pace: CrawlPace) -> Self {
        Self {
            crawl_root_url: crawl_root_url.into(),
            pace,
            seeds: CrawlSeeds::default(),
        }
    }
}

/// What an ended crawl left for the next one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrawlDiscovery {
    /// URLs the crawl found or was seeded with but did not visit, at most [`MAX_FRONTIER_URLS`].
    pub frontier: Vec<Url>,
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

    /// The admitted seeds, each once and never the root itself, at most [`MAX_FRONTIER_URLS`].
    pub(super) fn admit_all<'u>(&self, urls: impl IntoIterator<Item = &'u Url>) -> Vec<String> {
        let mut seen = HashSet::new();
        seen.insert(self.rules.root.as_str().to_string());
        urls.into_iter()
            .filter_map(|url| self.admit(url))
            .map(|stored| stored.as_url().as_str().to_string())
            .filter(|stored| seen.insert(stored.clone()))
            .take(MAX_FRONTIER_URLS)
            .collect()
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
}

impl FoundLinks {
    /// Records the links Spider follows by `rules`.
    pub(super) fn new(rules: Arc<FollowRules>) -> Self {
        Self {
            rules,
            links: Arc::default(),
            delivered: Arc::default(),
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

    /// The seeds and found links whose page the crawl neither delivered nor `visited` counts,
    /// each once, at most [`MAX_FRONTIER_URLS`]. A page arrives in the spelling the crawler
    /// stores, which differs from the link Spider counts visited when the link carried
    /// parameters that do not identify the page.
    pub(super) fn unvisited(
        &self,
        seeds: &[String],
        visited: impl Fn(&CaseInsensitiveString) -> bool,
    ) -> Vec<String> {
        let found = self.snapshot();
        let delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        unvisited(
            seeds.iter().chain(found.iter()),
            |link| delivered.contains(link) || visited(link),
            MAX_FRONTIER_URLS,
        )
    }

    /// The seeds and found links whose page the crawl neither delivered nor already knew, at
    /// most [`MAX_FRONTIER_URLS`].
    pub(super) fn frontier(
        &self,
        seeds: &[String],
        known: &HashSet<CaseInsensitiveString>,
    ) -> Vec<Url> {
        self.unvisited(seeds, |link| known.contains(link))
            .iter()
            .filter_map(|url| Url::parse(url).ok())
            .collect()
    }

    /// How many pages of `known` the delivered pages link that the crawl did not deliver, each
    /// page once whatever its trailing slash.
    pub(super) fn skipped_known(&self, known: &HashSet<CaseInsensitiveString>) -> usize {
        let delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        self.snapshot()
            .iter()
            .filter(|link| {
                let link = CaseInsensitiveString::from(link.as_str());
                known.contains(&link) && !delivered.contains(&link)
            })
            .filter_map(|link| Url::parse(link).ok())
            .map(|url| CrawledUrl::new(url).page_spelling())
            .collect::<HashSet<_>>()
            .len()
    }

    /// Records the links of the page at `page_url`, written as the page writes them: Spider
    /// reports them unresolved.
    pub(super) fn record<'a>(
        &self,
        page_url: &str,
        page_links: impl IntoIterator<Item = &'a CaseInsensitiveString>,
    ) {
        let Ok(page_url) = Url::parse(page_url) else {
            return;
        };
        let stored: Vec<String> = page_links
            .into_iter()
            .filter_map(|link| self.stored_spelling(&page_url, link.inner()))
            .collect();
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

    /// The link as Spider would request it (`FoundLinkDecoder`), when Spider follows it.
    fn stored_spelling(&self, page_url: &Url, link: &str) -> Option<String> {
        let decoded = html_escape::decode_html_entities(link);
        let resolved = page_url.join(decoded.trim()).ok()?;
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

/// Every spelling under which Spider may meet a page the crawl already knows: a page linked
/// with and without its trailing slash is one page (`docs/crawler/networking.md#stored-urls`).
pub(super) fn known_spellings<'a>(
    known: impl IntoIterator<Item = &'a Url>,
) -> Vec<CaseInsensitiveString> {
    known
        .into_iter()
        .flat_map(|url| CrawledUrl::new(url.clone()).page_spellings())
        .map(|url| CaseInsensitiveString::from(url.as_str()))
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
            admission.admit_all(&seeds),
            [
                "https://shop.example/product/1",
                "https://shop.example/product/6"
            ]
        );
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

        found.record("https://shop.example/catalogue/", &links);

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
    fn should_count_each_known_page_linked_but_not_delivered_once() {
        let found = FoundLinks::new(Arc::new(follow_rules(r"/denied")));
        let links: Vec<CaseInsensitiveString> = [
            "/product/1",
            "/product/1/",
            "/product/2",
            "/product/3",
            "/category/1",
        ]
        .into_iter()
        .map(CaseInsensitiveString::from)
        .collect();
        found.record("https://shop.example/", &links);
        found.record_delivered("https://shop.example/product/2");
        let known: HashSet<CaseInsensitiveString> =
            known_spellings(&[url("/product/1"), url("/product/2"), url("/product/9")])
                .into_iter()
                .collect();

        assert_eq!(found.skipped_known(&known), 1);
    }

    #[test]
    fn should_know_a_page_under_both_trailing_slash_spellings() {
        let spellings = known_spellings(&[url("/product/1/")]);

        assert!(spellings.contains(&CaseInsensitiveString::from(
            "https://shop.example/product/1"
        )));
        assert!(spellings.contains(&CaseInsensitiveString::from(
            "https://shop.example/product/1/"
        )));
    }
}
