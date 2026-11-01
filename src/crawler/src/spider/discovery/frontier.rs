//! What a crawl visits besides the links it finds from its root, and what it leaves unvisited
//! (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
//!
//! Link discovery from the root stays the authority on what a shop lists. Seeds, the URLs an
//! earlier cut-short crawl left unvisited, only extend it: they are fetched, filtered and stored
//! like found links, within the same crawl budget.

use regex::RegexSet;
use spider::CaseInsensitiveString;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use url::Url;

use super::robots::CrawlRobots;
use super::website_spider::CrawlPace;
use crate::network::policy::is_same_or_www_host;
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

/// Seeds a crawl may hand Spider: on the crawl root's host, in the spelling the crawler stores,
/// not refused by the URL blacklist, robots.txt or Spider's deny list.
pub(super) struct SeedAdmission<'a> {
    pub(super) root: &'a Url,
    pub(super) robots: &'a CrawlRobots,
    pub(super) deny_list: &'a RegexSet,
}

impl SeedAdmission<'_> {
    pub(super) fn admit(&self, url: &Url) -> Option<CrawledUrl> {
        let stored = CrawledUrl::new(url.clone());
        let admitted = is_same_or_www_host(self.root, stored.as_url())
            && !stored.is_blacklisted()
            && self.robots.allows(stored.as_url())
            && !self.deny_list.is_match(stored.as_url().as_str());
        admitted.then_some(stored)
    }

    /// The admitted seeds, each once and never the root itself, at most [`MAX_FRONTIER_URLS`].
    pub(super) fn admit_all<'u>(&self, urls: impl IntoIterator<Item = &'u Url>) -> Vec<String> {
        let mut seen = HashSet::new();
        seen.insert(self.root.as_str().to_string());
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
    root: Arc<Url>,
    deny_list: Arc<RegexSet>,
    links: Arc<Mutex<HashSet<String>>>,
    /// The pages Spider delivered. Spider counts a link visited once it starts
    /// the fetch, also when its crawl ended before the page arrived.
    delivered: Arc<Mutex<HashSet<CaseInsensitiveString>>>,
}

impl FoundLinks {
    /// Links on `root`'s host that neither the URL blacklist nor `deny_list` refuses.
    pub(super) fn new(root: Url, deny_list: RegexSet) -> Self {
        Self {
            root: Arc::new(root),
            deny_list: Arc::new(deny_list),
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

    /// The seeds and found links whose page the crawl neither delivered nor already knew, at
    /// most [`MAX_FRONTIER_URLS`].
    pub(super) fn frontier(
        &self,
        seeds: &[String],
        known: &HashSet<CaseInsensitiveString>,
    ) -> Vec<Url> {
        let found = self.snapshot();
        let delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        unvisited(
            seeds.iter().chain(found.iter()),
            |link| delivered.contains(link) || known.contains(link),
            MAX_FRONTIER_URLS,
        )
        .iter()
        .filter_map(|url| Url::parse(url).ok())
        .collect()
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

    /// The link as Spider would request it (`FoundLinkDecoder`), when the crawl may follow it.
    fn stored_spelling(&self, page_url: &Url, link: &str) -> Option<String> {
        let decoded = html_escape::decode_html_entities(link);
        let stored = CrawledUrl::new(page_url.join(decoded.trim()).ok()?);
        let followed = is_same_or_www_host(&self.root, stored.as_url())
            && !stored.is_blacklisted()
            && !self.deny_list.is_match(stored.as_url().as_str());
        followed.then(|| stored.as_url().as_str().to_string())
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
pub(super) fn known_spellings(known: &[Url]) -> Vec<CaseInsensitiveString> {
    known
        .iter()
        .flat_map(|url| CrawledUrl::new(url.clone()).page_spellings())
        .map(|url| CaseInsensitiveString::from(url.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::robots::{DenyList, RobotsRules};

    fn url(path: &str) -> Url {
        Url::parse(&format!("https://shop.example{path}")).unwrap()
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
    fn should_admit_seeds_on_the_root_host_that_robots_and_url_rules_allow() {
        let root = url("/");
        let robots = robots("User-agent: *\nDisallow: /private\n");
        let deny_list = RegexSet::new([r"/denied"]).unwrap();
        let admission = SeedAdmission {
            root: &root,
            robots: &robots,
            deny_list: &deny_list,
        };
        let seeds = [
            url("/product/1?utm_source=newsletter"),
            url("/product/1"),
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
                "https://www.shop.example/product/2"
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
    fn should_record_page_links_the_crawl_may_follow_in_their_stored_spelling() {
        let deny_list = RegexSet::new([r"/denied"]).unwrap();
        let found = FoundLinks::new(url("/"), deny_list);
        let links: Vec<CaseInsensitiveString> = [
            "item.php?id=1&amp;utm_source=x",
            "https://www.shop.example/product/2",
            "https://other.example/product/3",
            "https://shop.example/denied/4",
            "https://shop.example/cart",
        ]
        .into_iter()
        .map(CaseInsensitiveString::from)
        .collect();

        found.record("https://shop.example/catalogue/", &links);

        assert_eq!(
            found.snapshot(),
            [
                "https://shop.example/catalogue/item.php?id=1".to_string(),
                "https://www.shop.example/product/2".to_string()
            ]
            .into_iter()
            .collect()
        );
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
