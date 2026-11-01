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

/// The links Spider found on crawled pages, in the spelling it requests them.
#[derive(Clone, Default)]
pub(super) struct FoundLinks(Arc<Mutex<HashSet<String>>>);

impl FoundLinks {
    pub(super) fn record(&self, link: &str) {
        let mut found = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if found.len() < MAX_FOUND_LINKS {
            found.insert(link.to_string());
        }
    }

    pub(super) fn snapshot(&self) -> HashSet<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
