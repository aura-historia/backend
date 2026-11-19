//! Reads a shop's sitemaps within a crawl's bounds: from the files robots.txt names, or the
//! first conventional path that is a sitemap, one index level down, under caps on requests,
//! bytes and URLs, and within its share of the crawl's time
//! (`docs/crawler/networking.md#sitemaps`). Nothing a sitemap does fails the crawl: every
//! failure is recorded in the [diagnostics](SitemapDiagnostics) and the crawl goes on with what
//! was read.

use std::collections::HashSet;
use std::io::Read;
use std::time::Duration;

use tokio::time::Instant;
use url::Url;

use super::locations::{SitemapSource, on_shop_host, products_first, starting_sitemaps};
use super::parser::{SitemapKind, SitemapParseError, parse_sitemap};
use crate::network::policy::NetworkErrorKind;
use crate::spider::discovery::page_status::RateLimitSignal;
use crate::spider::discovery::robots::CrawlRobots;
use crate::spider::utils::url::CrawledUrl;

/// Redirects one sitemap request may follow, each hop a request of its own.
const MAX_SITEMAP_REDIRECTS: usize = 3;

/// The bounds of one crawl's sitemap reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SitemapLimits {
    /// Sitemap requests per crawl, probes, index files and redirect hops included.
    pub max_requests: usize,
    /// Decoded bytes of one sitemap file; the sitemaps.org limit is 50 MB.
    pub max_file_bytes: usize,
    /// Entries read from one file; the sitemaps.org limit is 50,000.
    pub max_urls_per_file: usize,
    /// Distinct page URLs one crawl takes from its sitemaps.
    pub max_urls: usize,
    /// The share (one over the value) of the crawl's wall-clock budget sitemap reading may take,
    /// so the crawl keeps the rest for its pages.
    pub budget_divisor: u32,
}

impl Default for SitemapLimits {
    fn default() -> Self {
        Self {
            max_requests: 60,
            max_file_bytes: 50 * 1024 * 1024,
            max_urls_per_file: 50_000,
            max_urls: 100_000,
            budget_divisor: 4,
        }
    }
}

/// Why one sitemap request or file gave the crawl nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitemapFailureKind {
    /// The request failed without an answer: timeout, connect error, a TLS failure, an address
    /// the destination policy refuses.
    Transport(NetworkErrorKind),
    /// The certificate or TLS handshake failed, such as a certificate for another name.
    Tls,
    /// The shop answered with a status other than success or a followed redirect.
    Status(u16),
    /// A rate-limit answer; reading stops and the crawl ends as rate-limited.
    RateLimited(u16),
    /// The body is no sitemap, such as an RSS feed or an HTML page.
    NotASitemap,
    /// The body is not well-formed XML, or its compression is corrupt.
    Malformed,
    /// The body is larger than [`SitemapLimits::max_file_bytes`].
    TooLarge,
    /// robots.txt disallows the sitemap's URL.
    RobotsDisallowed,
    /// The sitemap or a redirect to it is not on the crawl root's host or its `www` twin, or is
    /// no URL at all.
    OffHost,
    /// More redirects than [`MAX_SITEMAP_REDIRECTS`], or one without a usable `Location`.
    RedirectRefused,
    /// A sitemap index listed by an index; only one level is followed.
    NestedIndex,
}

impl SitemapFailureKind {
    /// Stable label for the crawl diagnostics; not persisted.
    pub fn label(self) -> String {
        match self {
            Self::Transport(kind) => format!("transport_{}", kind.as_str().to_ascii_lowercase()),
            Self::Tls => "tls".to_owned(),
            Self::Status(status) => format!("status_{status}"),
            Self::RateLimited(status) => format!("rate_limited_{status}"),
            Self::NotASitemap => "not_a_sitemap".to_owned(),
            Self::Malformed => "malformed".to_owned(),
            Self::TooLarge => "too_large".to_owned(),
            Self::RobotsDisallowed => "robots_disallowed".to_owned(),
            Self::OffHost => "off_host".to_owned(),
            Self::RedirectRefused => "redirect_refused".to_owned(),
            Self::NestedIndex => "nested_index".to_owned(),
        }
    }
}

/// One sitemap that gave the crawl nothing, in the [stored spelling](CrawledUrl), which drops
/// credentials and session parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitemapFailure {
    pub url: String,
    pub kind: SitemapFailureKind,
}

impl std::fmt::Display for SitemapFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.url, self.kind.label())
    }
}

/// What a crawl's sitemap reading did. Bounded by [`SitemapLimits`]: at most one failure per
/// request or listed sitemap, and counters only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SitemapDiagnostics {
    /// Where the crawl looked; `None` when it did not read sitemaps.
    pub source: Option<SitemapSource>,
    /// Requests sent, redirect hops included.
    pub requests: usize,
    /// Files read as a sitemap or sitemap index.
    pub files_read: usize,
    /// Sitemaps the request cap or the time share left unread.
    pub files_skipped: usize,
    /// Files with more entries than the per-file limit, of which the rest was not read.
    pub files_truncated: usize,
    /// Page URLs the files listed, entries without a readable location excluded.
    pub urls_listed: usize,
    /// Listed URLs that are no URL, or not on the crawl root's host or its `www` twin.
    pub urls_off_host: usize,
    /// Listed URLs dropped once the crawl took [`SitemapLimits::max_urls`].
    pub urls_over_limit: usize,
    /// Listed URLs the crawl refused as a found link it would not follow: the URL blacklist,
    /// robots.txt or another scheme or port than the root's.
    pub urls_refused: usize,
    /// URLs the ListingSource's product URL pattern takes, stored without a fetch.
    pub products_registered: usize,
    /// Product URLs the ListingSource already stores, which were not registered again.
    pub products_known: usize,
    /// Product URLs listed with a language segment before their path that the sitemaps also
    /// list without it, which were not registered.
    pub products_language_copies: usize,
    /// URLs handed to the crawl as seeds.
    pub seeds: usize,
    pub failures: Vec<SitemapFailure>,
    /// Wall-clock time sitemap reading took from the crawl's budget.
    pub elapsed_ms: u64,
}

impl SitemapDiagnostics {
    fn fail(&mut self, url: &Url, kind: SitemapFailureKind) {
        self.failures.push(SitemapFailure {
            url: CrawledUrl::new(url.clone()).to_string(),
            kind,
        });
    }

    /// The failures as `url (kind)` labels, for logs.
    pub fn failure_labels(&self) -> Vec<String> {
        self.failures.iter().map(ToString::to_string).collect()
    }
}

/// The page URLs a crawl's sitemaps list, on the crawl root's host: a URL listed on its `www`
/// twin is moved to the root's host, the host the crawl stores its pages under. Each once, in
/// the order read.
#[derive(Debug, Default)]
pub struct SitemapListing {
    pub pages: Vec<Url>,
    pub diagnostics: SitemapDiagnostics,
    /// The rate-limit answer that stopped the reading; the crawl stops on it as on a page's.
    pub rate_limit: Option<RateLimitSignal>,
}

/// One answer to a sitemap request.
#[derive(Debug, Clone)]
pub(in crate::spider::discovery) enum SitemapAnswer {
    /// A success with its body, decoded from its content coding within the byte limit.
    Body(Vec<u8>),
    /// A redirect with its `Location` text, when it had a readable one.
    Redirect(Option<String>),
    RateLimited(RateLimitSignal),
    Status(u16),
    Failed(SitemapFailureKind),
}

/// Sends one sitemap request through the crawl's fetch path: at the crawl's pace, within its
/// politeness group, under the destination policy.
#[async_trait::async_trait]
pub(in crate::spider::discovery) trait SitemapFetcher:
    Send + Sync
{
    async fn fetch(&self, url: &Url) -> SitemapAnswer;
}

/// What a crawl from `root` reads its sitemaps under.
pub(in crate::spider::discovery) struct SitemapReading<'a> {
    pub root: &'a Url,
    pub robots: &'a CrawlRobots,
    pub limits: SitemapLimits,
    /// When sitemap reading must have ended, to leave the crawl the rest of its budget.
    pub deadline: Instant,
}

/// One sitemap file to read, and whether an index listed it.
struct PendingFile {
    url: Url,
    listed_by_index: bool,
}

/// Reads the sitemaps of `reading.root` through `fetcher`.
pub(in crate::spider::discovery) async fn read_sitemaps(
    reading: SitemapReading<'_>,
    fetcher: &dyn SitemapFetcher,
) -> SitemapListing {
    let started = Instant::now();
    let robots_records = reading
        .robots
        .rules
        .as_ref()
        .map(|rules| rules.sitemaps())
        .unwrap_or_default();
    let (source, starting) = starting_sitemaps(reading.root, robots_records);
    let mut reader = Reader {
        reading: &reading,
        fetcher,
        listing: SitemapListing::default(),
        files_seen: HashSet::new(),
        pages_seen: HashSet::new(),
        stopped: false,
    };
    reader.listing.diagnostics.source = Some(source);

    let mut starting_files = Vec::new();
    for raw in starting {
        match reading.root.join(&raw) {
            Ok(url) if on_shop_host(reading.root, &url) => starting_files.push(PendingFile {
                url,
                listed_by_index: false,
            }),
            Ok(url) => reader
                .listing
                .diagnostics
                .fail(&url, SitemapFailureKind::OffHost),
            Err(_) => reader.listing.diagnostics.files_skipped += 1,
        }
    }
    let children = match source {
        SitemapSource::RobotsTxt => reader.read_all(starting_files).await,
        SitemapSource::Probe => reader.read_first_sitemap(starting_files).await,
    };
    reader.read_all(products_first_files(children)).await;

    let mut listing = reader.listing;
    listing.diagnostics.elapsed_ms =
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    listing
}

fn products_first_files(children: Vec<Url>) -> Vec<PendingFile> {
    products_first(children)
        .into_iter()
        .map(|url| PendingFile {
            url,
            listed_by_index: true,
        })
        .collect()
}

/// What one sitemap file gave.
enum ReadFile {
    /// A sitemap or an index; an index's children are returned to be read next.
    Sitemap {
        children: Vec<Url>,
    },
    Nothing,
}

struct Reader<'r, 'a> {
    reading: &'r SitemapReading<'a>,
    fetcher: &'r dyn SitemapFetcher,
    listing: SitemapListing,
    files_seen: HashSet<String>,
    pages_seen: HashSet<String>,
    /// A rate-limit answer, the request cap or the deadline ended the reading.
    stopped: bool,
}

impl Reader<'_, '_> {
    /// Reads every file, and returns the children the indexes among them list.
    async fn read_all(&mut self, files: Vec<PendingFile>) -> Vec<Url> {
        let mut children = Vec::new();
        for file in files {
            if let ReadFile::Sitemap { children: listed } = self.read_file(file).await {
                children.extend(listed);
            }
        }
        children
    }

    /// Reads the files in order until one is a sitemap, and returns the children it lists.
    async fn read_first_sitemap(&mut self, files: Vec<PendingFile>) -> Vec<Url> {
        for file in files {
            if let ReadFile::Sitemap { children } = self.read_file(file).await {
                return children;
            }
        }
        Vec::new()
    }

    async fn read_file(&mut self, file: PendingFile) -> ReadFile {
        if self.stopped {
            self.listing.diagnostics.files_skipped += 1;
            return ReadFile::Nothing;
        }
        if !self.files_seen.insert(file.url.as_str().to_owned()) {
            return ReadFile::Nothing;
        }
        let Some((url, body)) = self.fetch_following_redirects(file.url).await else {
            return ReadFile::Nothing;
        };
        let limits = self.reading.limits;
        let body = match decompressed(body, limits.max_file_bytes) {
            Ok(body) => body,
            Err(kind) => {
                self.listing.diagnostics.fail(&url, kind);
                return ReadFile::Nothing;
            }
        };
        let parsed = match parse_sitemap(&body, limits.max_urls_per_file) {
            Ok(parsed) => parsed,
            Err(error) => {
                let kind = match error {
                    SitemapParseError::NotASitemap { .. } | SitemapParseError::NoDocument => {
                        SitemapFailureKind::NotASitemap
                    }
                    SitemapParseError::Malformed(_) | SitemapParseError::Unterminated => {
                        SitemapFailureKind::Malformed
                    }
                };
                self.listing.diagnostics.fail(&url, kind);
                return ReadFile::Nothing;
            }
        };
        let diagnostics = &mut self.listing.diagnostics;
        diagnostics.files_read += 1;
        diagnostics.files_truncated += usize::from(parsed.truncated);
        match parsed.kind {
            SitemapKind::Index if file.listed_by_index => {
                diagnostics.fail(&url, SitemapFailureKind::NestedIndex);
                ReadFile::Nothing
            }
            SitemapKind::Index => ReadFile::Sitemap {
                children: self.index_children(&url, &parsed.locations),
            },
            SitemapKind::UrlSet => {
                self.take_pages(&parsed.locations);
                ReadFile::Sitemap {
                    children: Vec::new(),
                }
            }
        }
    }

    /// The child sitemaps an index at `index` lists, on the shop's host.
    fn index_children(&mut self, index: &Url, locations: &[String]) -> Vec<Url> {
        let mut children = Vec::new();
        for location in locations {
            match index.join(location) {
                Ok(child) if on_shop_host(self.reading.root, &child) => children.push(child),
                Ok(child) => self
                    .listing
                    .diagnostics
                    .fail(&child, SitemapFailureKind::OffHost),
                Err(_) => self.listing.diagnostics.files_skipped += 1,
            }
        }
        children
    }

    /// Takes the listed pages on the shop's host, moved to the root's host, each once, up to the
    /// crawl's URL limit. A page must be listed by its absolute URL.
    fn take_pages(&mut self, locations: &[String]) {
        let root = self.reading.root;
        let max_urls = self.reading.limits.max_urls;
        let diagnostics = &mut self.listing.diagnostics;
        diagnostics.urls_listed += locations.len();
        for location in locations {
            let Some(page) = Url::parse(location)
                .ok()
                .filter(|url| on_shop_host(root, url))
                .and_then(|url| on_root_host(root, url))
            else {
                diagnostics.urls_off_host += 1;
                continue;
            };
            if self.pages_seen.len() >= max_urls {
                diagnostics.urls_over_limit += 1;
                continue;
            }
            if self.pages_seen.insert(page.as_str().to_owned()) {
                self.listing.pages.push(page);
            }
        }
    }

    /// The body of the sitemap at `url`, after at most [`MAX_SITEMAP_REDIRECTS`] redirects on
    /// the shop's host that robots.txt allows, with the URL it was read from; `None` once a
    /// failure is recorded.
    async fn fetch_following_redirects(&mut self, mut url: Url) -> Option<(Url, Vec<u8>)> {
        for hop in 0..=MAX_SITEMAP_REDIRECTS {
            if !self.reading.robots.allows(&url) {
                self.listing
                    .diagnostics
                    .fail(&url, SitemapFailureKind::RobotsDisallowed);
                return None;
            }
            let answer = self.fetch(&url).await?;
            match answer {
                SitemapAnswer::Body(body) => return Some((url, body)),
                SitemapAnswer::Redirect(Some(location)) if hop < MAX_SITEMAP_REDIRECTS => {
                    match url.join(location.trim()) {
                        Ok(target) if on_shop_host(self.reading.root, &target) => url = target,
                        _ => {
                            self.listing
                                .diagnostics
                                .fail(&url, SitemapFailureKind::OffHost);
                            return None;
                        }
                    }
                }
                SitemapAnswer::Redirect(_) => {
                    self.listing
                        .diagnostics
                        .fail(&url, SitemapFailureKind::RedirectRefused);
                    return None;
                }
                SitemapAnswer::RateLimited(signal) => {
                    self.listing
                        .diagnostics
                        .fail(&url, SitemapFailureKind::RateLimited(signal.status));
                    self.listing.rate_limit = Some(signal);
                    self.stopped = true;
                    return None;
                }
                SitemapAnswer::Status(status) => {
                    self.listing
                        .diagnostics
                        .fail(&url, SitemapFailureKind::Status(status));
                    return None;
                }
                SitemapAnswer::Failed(kind) => {
                    self.listing.diagnostics.fail(&url, kind);
                    return None;
                }
            }
        }
        None
    }

    /// One request within the request cap and before the deadline; `None` when neither leaves
    /// room, which ends the reading.
    async fn fetch(&mut self, url: &Url) -> Option<SitemapAnswer> {
        let limits = self.reading.limits;
        if self.listing.diagnostics.requests >= limits.max_requests
            || Instant::now() >= self.reading.deadline
        {
            self.stop();
            return None;
        }
        self.listing.diagnostics.requests += 1;
        match tokio::time::timeout_at(self.reading.deadline, self.fetcher.fetch(url)).await {
            Ok(answer) => Some(answer),
            Err(_) => {
                self.stop();
                None
            }
        }
    }

    fn stop(&mut self) {
        self.stopped = true;
        self.listing.diagnostics.files_skipped += 1;
    }
}

/// `page` on the root's host: a page listed on the root's `www` twin is the page the crawl
/// stores on the root's host.
fn on_root_host(root: &Url, mut page: Url) -> Option<Url> {
    let root_host = root.host_str()?;
    if page.host_str() != Some(root_host) {
        page.set_host(Some(root_host)).ok()?;
    }
    Some(page)
}

/// The gzip magic bytes; a sitemap may be served as a `.xml.gz` file.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// `body` decompressed when it is a gzip file, within `limit` decoded bytes.
fn decompressed(body: Vec<u8>, limit: usize) -> Result<Vec<u8>, SitemapFailureKind> {
    if !body.starts_with(&GZIP_MAGIC) {
        return Ok(body);
    }
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(body.as_slice())
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut decoded)
        .map_err(|_| SitemapFailureKind::Malformed)?;
    if decoded.len() > limit {
        return Err(SitemapFailureKind::TooLarge);
    }
    Ok(decoded)
}

/// The time a crawl with `budget` gives its sitemap reading.
pub(in crate::spider::discovery) fn sitemap_time_share(
    budget: Duration,
    limits: SitemapLimits,
) -> Duration {
    budget / limits.budget_divisor.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::robots::{DenyList, RobotsRules};
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::Mutex;

    /// Answers each URL from a map, `404` for any other, and logs every request.
    #[derive(Default)]
    struct FakeShop {
        answers: HashMap<String, SitemapAnswer>,
        requested: Mutex<Vec<String>>,
        delay: Duration,
    }

    impl FakeShop {
        fn with(mut self, url: &str, answer: SitemapAnswer) -> Self {
            self.answers.insert(url.to_owned(), answer);
            self
        }

        fn xml(self, url: &str, body: &str) -> Self {
            self.with(url, SitemapAnswer::Body(body.as_bytes().to_vec()))
        }

        fn requested(&self) -> Vec<String> {
            self.requested.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl SitemapFetcher for FakeShop {
        async fn fetch(&self, url: &Url) -> SitemapAnswer {
            self.requested.lock().unwrap().push(url.to_string());
            tokio::time::sleep(self.delay).await;
            self.answers
                .get(url.as_str())
                .cloned()
                .unwrap_or(SitemapAnswer::Status(404))
        }
    }

    const ROOT: &str = "https://www.shop.example/";

    fn robots(body: &str) -> CrawlRobots {
        let rules = RobotsRules::parse(body);
        CrawlRobots {
            deny_list: DenyList::default(),
            crawl_delay: None,
            rules: Some(rules),
            policy: "rules",
        }
    }

    fn url_set(paths: &[&str]) -> String {
        let entries = paths
            .iter()
            .map(|path| format!("<url><loc>{path}</loc></url>"))
            .collect::<String>();
        format!("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">{entries}</urlset>")
    }

    fn index(children: &[&str]) -> String {
        let entries = children
            .iter()
            .map(|child| format!("<sitemap><loc>{child}</loc></sitemap>"))
            .collect::<String>();
        format!("<sitemapindex>{entries}</sitemapindex>")
    }

    async fn read_with(shop: &FakeShop, robots_txt: &str, limits: SitemapLimits) -> SitemapListing {
        let root = Url::parse(ROOT).unwrap();
        let robots = robots(robots_txt);
        read_sitemaps(
            SitemapReading {
                root: &root,
                robots: &robots,
                limits,
                deadline: Instant::now() + Duration::from_secs(60),
            },
            shop,
        )
        .await
    }

    async fn read(shop: &FakeShop, robots_txt: &str) -> SitemapListing {
        read_with(shop, robots_txt, SitemapLimits::default()).await
    }

    fn pages(listing: &SitemapListing) -> Vec<&str> {
        listing.pages.iter().map(Url::as_str).collect()
    }

    fn failures(listing: &SitemapListing) -> Vec<String> {
        listing.diagnostics.failure_labels()
    }

    #[tokio::test]
    async fn should_read_the_sitemaps_robots_txt_names_and_follow_an_index_one_level() {
        let shop = FakeShop::default()
            .xml(
                "https://www.shop.example/feeds/sitemap.xml",
                &index(&[
                    "https://www.shop.example/sitemap-pages.xml",
                    "https://www.shop.example/sitemap-products.xml",
                ]),
            )
            .xml(
                "https://www.shop.example/sitemap-products.xml",
                &url_set(&[
                    "https://www.shop.example/products/a",
                    "https://shop.example/products/b",
                ]),
            )
            .xml(
                "https://www.shop.example/sitemap-pages.xml",
                &url_set(&["https://www.shop.example/about"]),
            );

        let listing = read(
            &shop,
            "Sitemap: https://www.shop.example/feeds/sitemap.xml\n",
        )
        .await;

        assert_eq!(
            pages(&listing),
            [
                "https://www.shop.example/products/a",
                "https://www.shop.example/products/b",
                "https://www.shop.example/about",
            ]
        );
        assert_eq!(
            shop.requested(),
            [
                "https://www.shop.example/feeds/sitemap.xml",
                "https://www.shop.example/sitemap-products.xml",
                "https://www.shop.example/sitemap-pages.xml",
            ]
        );
        let diagnostics = &listing.diagnostics;
        assert_eq!(diagnostics.source, Some(SitemapSource::RobotsTxt));
        assert_eq!((diagnostics.requests, diagnostics.files_read), (3, 3));
        assert_eq!(diagnostics.urls_listed, 3);
        assert!(diagnostics.failures.is_empty());
    }

    #[tokio::test]
    async fn should_probe_until_a_conventional_path_is_a_sitemap() {
        let shop = FakeShop::default()
            .xml(
                "https://www.shop.example/sitemap.xml",
                "<rss><channel/></rss>",
            )
            .xml(
                "https://www.shop.example/sitemap_index.xml",
                &url_set(&["https://www.shop.example/product/1"]),
            );

        let listing = read(&shop, "User-agent: *\nDisallow: /cart\n").await;

        assert_eq!(pages(&listing), ["https://www.shop.example/product/1"]);
        assert_eq!(shop.requested().len(), 2);
        assert_eq!(listing.diagnostics.source, Some(SitemapSource::Probe));
        assert_eq!(
            failures(&listing),
            ["https://www.shop.example/sitemap.xml (not_a_sitemap)"]
        );
    }

    #[tokio::test]
    async fn should_record_every_probe_a_shop_without_a_sitemap_answers() {
        let shop = FakeShop::default();

        let listing = read(&shop, "").await;

        assert!(listing.pages.is_empty());
        assert_eq!(listing.diagnostics.requests, 3);
        assert_eq!(listing.diagnostics.files_read, 0);
        assert_eq!(
            failures(&listing),
            [
                "https://www.shop.example/sitemap.xml (status_404)",
                "https://www.shop.example/sitemap_index.xml (status_404)",
                "https://www.shop.example/wp-sitemap.xml (status_404)",
            ]
        );
    }

    #[tokio::test]
    async fn should_neither_request_nor_list_anything_off_the_shops_host() {
        let shop = FakeShop::default()
            .xml(
                "https://www.shop.example/sitemap.xml",
                &index(&[
                    "https://cdn.example/sitemap-products.xml",
                    "https://www.shop.example/sitemap-1.xml",
                ]),
            )
            .xml(
                "https://www.shop.example/sitemap-1.xml",
                &url_set(&[
                    "https://www.shop.example/a",
                    "https://other.example/b",
                    "https://www.shop.example:8443/c",
                    "/relative",
                    "javascript:alert(1)",
                ]),
            );

        let listing = read(
            &shop,
            "Sitemap: https://www.shop.example/sitemap.xml\nSitemap: https://evil.example/sitemap.xml\n",
        )
        .await;

        assert_eq!(pages(&listing), ["https://www.shop.example/a"]);
        assert_eq!(listing.diagnostics.urls_off_host, 4);
        assert!(
            shop.requested()
                .iter()
                .all(|requested| requested.starts_with("https://www.shop.example/"))
        );
        assert_eq!(
            failures(&listing),
            [
                "https://evil.example/sitemap.xml (off_host)",
                "https://cdn.example/sitemap-products.xml (off_host)",
            ]
        );
    }

    #[tokio::test]
    async fn should_not_request_a_sitemap_robots_txt_disallows() {
        let shop = FakeShop::default();

        let listing = read(
            &shop,
            "User-agent: *\nDisallow: /private/\nSitemap: https://www.shop.example/private/sitemap.xml\n",
        )
        .await;

        assert!(shop.requested().is_empty());
        assert_eq!(
            failures(&listing),
            ["https://www.shop.example/private/sitemap.xml (robots_disallowed)"]
        );
    }

    #[tokio::test]
    async fn should_follow_a_redirect_on_the_shops_host_and_refuse_one_off_it() {
        let shop = FakeShop::default()
            .with(
                "https://www.shop.example/sitemap.xml",
                SitemapAnswer::Redirect(Some("/sitemap_index.xml".to_owned())),
            )
            .xml(
                "https://www.shop.example/sitemap_index.xml",
                &url_set(&["https://www.shop.example/p/1"]),
            )
            .with(
                "https://www.shop.example/moved.xml",
                SitemapAnswer::Redirect(Some("https://elsewhere.example/sitemap.xml".to_owned())),
            );

        let listing = read(
            &shop,
            "Sitemap: https://www.shop.example/sitemap.xml\nSitemap: https://www.shop.example/moved.xml\n",
        )
        .await;

        assert_eq!(pages(&listing), ["https://www.shop.example/p/1"]);
        assert_eq!(listing.diagnostics.requests, 3);
        assert_eq!(
            failures(&listing),
            ["https://www.shop.example/moved.xml (off_host)"]
        );
    }

    #[tokio::test]
    async fn should_not_follow_an_index_listed_by_an_index() {
        let shop = FakeShop::default()
            .xml(
                "https://www.shop.example/sitemap.xml",
                &index(&["https://www.shop.example/nested.xml"]),
            )
            .xml(
                "https://www.shop.example/nested.xml",
                &index(&["https://www.shop.example/deeper.xml"]),
            );

        let listing = read(&shop, "").await;

        assert_eq!(shop.requested().len(), 2);
        assert_eq!(
            failures(&listing),
            ["https://www.shop.example/nested.xml (nested_index)"]
        );
    }

    #[tokio::test]
    async fn should_read_product_sitemaps_first_within_the_request_cap() {
        let shop = FakeShop::default()
            .xml(
                "https://www.shop.example/sitemap_index.xml",
                &index(&[
                    "https://www.shop.example/pa_colour-sitemap.xml",
                    "https://www.shop.example/pa_size-sitemap.xml",
                    "https://www.shop.example/product-sitemap.xml",
                ]),
            )
            .xml(
                "https://www.shop.example/product-sitemap.xml",
                &url_set(&["https://www.shop.example/product/chair/"]),
            );

        let listing = read_with(
            &shop,
            "Sitemap: https://www.shop.example/sitemap_index.xml\n",
            SitemapLimits {
                max_requests: 2,
                ..SitemapLimits::default()
            },
        )
        .await;

        assert_eq!(pages(&listing), ["https://www.shop.example/product/chair/"]);
        assert_eq!(listing.diagnostics.requests, 2);
        assert_eq!(listing.diagnostics.files_skipped, 2);
    }

    #[tokio::test]
    async fn should_stop_reading_on_a_rate_limit_answer() {
        let signal = RateLimitSignal {
            status: 429,
            retry_after: Some(Duration::from_secs(30)),
        };
        let shop = FakeShop::default()
            .with(
                "https://www.shop.example/a.xml",
                SitemapAnswer::RateLimited(signal),
            )
            .xml(
                "https://www.shop.example/b.xml",
                &url_set(&["https://www.shop.example/x"]),
            );

        let listing = read(
            &shop,
            "Sitemap: https://www.shop.example/a.xml\nSitemap: https://www.shop.example/b.xml\n",
        )
        .await;

        assert_eq!(listing.rate_limit, Some(signal));
        assert_eq!(shop.requested(), ["https://www.shop.example/a.xml"]);
        assert!(listing.pages.is_empty());
        assert_eq!(listing.diagnostics.files_skipped, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn should_stop_reading_at_its_deadline() {
        let shop = FakeShop {
            delay: Duration::from_secs(40),
            ..FakeShop::default()
        }
        .xml(
            "https://www.shop.example/a.xml",
            &url_set(&["https://www.shop.example/x"]),
        )
        .xml(
            "https://www.shop.example/b.xml",
            &url_set(&["https://www.shop.example/y"]),
        );

        let listing = read(
            &shop,
            "Sitemap: https://www.shop.example/a.xml\nSitemap: https://www.shop.example/b.xml\n",
        )
        .await;

        assert_eq!(pages(&listing), ["https://www.shop.example/x"]);
        assert_eq!(shop.requested().len(), 2);
        assert_eq!(listing.diagnostics.files_skipped, 1);
    }

    #[tokio::test]
    async fn should_record_oversized_and_unanswered_sitemaps_and_go_on() {
        let shop = FakeShop::default()
            .with(
                "https://www.shop.example/big.xml",
                SitemapAnswer::Failed(SitemapFailureKind::TooLarge),
            )
            .with(
                "https://www.shop.example/tls.xml",
                SitemapAnswer::Failed(SitemapFailureKind::Tls),
            )
            .xml(
                "https://www.shop.example/ok.xml",
                &url_set(&["https://www.shop.example/kept"]),
            );

        let listing = read(
            &shop,
            "Sitemap: https://www.shop.example/big.xml\nSitemap: https://www.shop.example/tls.xml\nSitemap: https://www.shop.example/ok.xml\n",
        )
        .await;

        assert_eq!(pages(&listing), ["https://www.shop.example/kept"]);
        assert_eq!(
            failures(&listing),
            [
                "https://www.shop.example/big.xml (too_large)",
                "https://www.shop.example/tls.xml (tls)",
            ]
        );
    }

    #[tokio::test]
    async fn should_take_each_page_once_up_to_the_url_limit() {
        let shop = FakeShop::default().xml(
            "https://www.shop.example/sitemap.xml",
            &url_set(&[
                "https://www.shop.example/a",
                "https://shop.example/a",
                "https://www.shop.example/b",
                "https://www.shop.example/c",
            ]),
        );

        let listing = read_with(
            &shop,
            "",
            SitemapLimits {
                max_urls: 2,
                ..SitemapLimits::default()
            },
        )
        .await;

        assert_eq!(
            pages(&listing),
            ["https://www.shop.example/a", "https://www.shop.example/b"]
        );
        assert_eq!(listing.diagnostics.urls_over_limit, 1);
    }

    #[tokio::test]
    async fn should_read_a_gzip_sitemap_within_the_byte_limit() {
        let body = url_set(&["https://www.shop.example/zipped"]);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(body.as_bytes()).unwrap();
        let gzip = encoder.finish().unwrap();
        let shop = FakeShop::default()
            .with(
                "https://www.shop.example/sitemap.xml.gz",
                SitemapAnswer::Body(gzip.clone()),
            )
            .with(
                "https://www.shop.example/small.xml.gz",
                SitemapAnswer::Body(gzip),
            );

        let listing = read(&shop, "Sitemap: https://www.shop.example/sitemap.xml.gz\n").await;
        assert_eq!(pages(&listing), ["https://www.shop.example/zipped"]);

        assert_eq!(
            decompressed(
                shop.answers_body("https://www.shop.example/small.xml.gz"),
                10
            ),
            Err(SitemapFailureKind::TooLarge)
        );
        assert_eq!(
            decompressed(vec![0x1f, 0x8b, 0, 1, 2], 1_000),
            Err(SitemapFailureKind::Malformed)
        );
    }

    impl FakeShop {
        fn answers_body(&self, url: &str) -> Vec<u8> {
            match self.answers.get(url) {
                Some(SitemapAnswer::Body(body)) => body.clone(),
                _ => Vec::new(),
            }
        }
    }
}
