//! robots.txt rules for crawler requests, following RFC 9309.
//!
//! This module only parses and classifies; callers fetch `robots.txt` with
//! their own client and redirect policy, then ask the resulting
//! [`RobotsPolicy`] whether a URL may be requested and how far apart requests
//! must be.

use crate::network::policy::NetworkErrorKind;
use std::time::Duration;
use url::Url;

mod deny_list;
pub use deny_list::DenyList;

/// Upper bound for an honoured `Crawl-delay`; the Spider library caps the
/// delay it reads from robots.txt at the same value.
pub const MAX_CRAWL_DELAY: Duration = Duration::from_secs(60);

/// RFC 9309 requires parsing at least the first 500 KiB of a robots.txt file;
/// anything beyond the limit is ignored.
pub const MAX_ROBOTS_TXT_BYTES: usize = 512 * 1024;

/// RFC 9309 asks crawlers to follow at least five consecutive redirects,
/// even across authorities.
pub const MAX_ROBOTS_TXT_REDIRECTS: usize = 5;

/// The product token shops address the crawler by in `User-agent` lines.
const CRAWLER_PRODUCT_TOKEN: &str = "aurahistoriabot";

/// The robots.txt URL of `url`'s origin.
pub fn robots_txt_url(url: &Url) -> Url {
    let mut robots_url = url.clone();
    robots_url.set_path("/robots.txt");
    robots_url.set_query(None);
    robots_url.set_fragment(None);
    robots_url
}

/// One robots.txt answer: the policy it sets, or a redirect to follow.
#[derive(Debug, PartialEq)]
pub enum RobotsFetchStep {
    Policy(RobotsPolicy),
    Redirect(Url),
}

impl RobotsFetchStep {
    /// The step a non-success answer to `robots_url` gives: a redirect to
    /// its target, or the policy of its status. A redirect without a usable
    /// target leaves no robots.txt to read: RFC 9309 treats that as
    /// unavailable.
    pub fn for_unsuccessful(status: u16, location: Option<&str>, robots_url: &Url) -> Self {
        if !(300..400).contains(&status) {
            return Self::Policy(RobotsPolicy::for_failed_status(status));
        }
        match location.and_then(|location| robots_url.join(location).ok()) {
            Some(next) => Self::Redirect(next),
            None => Self::Policy(RobotsPolicy::Unavailable),
        }
    }
}

/// What robots.txt says about one origin.
#[derive(Debug, Clone, PartialEq)]
pub enum RobotsPolicy {
    /// robots.txt was served; the group that applies to the crawler decides.
    Rules(RobotsRules),
    /// robots.txt is unavailable (a 4xx answer other than 429, or an
    /// unusable redirect): every URL is allowed.
    Unavailable,
    /// robots.txt could not be reached (a 429 or 5xx answer): every URL is
    /// disallowed until robots.txt is fetched again.
    Unreachable,
}

/// Why robots.txt refuses a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobotsRefusal {
    Disallowed,
    Unreachable,
}

impl RobotsPolicy {
    /// The policy a served robots.txt body sets; only its first
    /// [`MAX_ROBOTS_TXT_BYTES`] count.
    pub fn from_body(body: &[u8]) -> Self {
        let body = &body[..body.len().min(MAX_ROBOTS_TXT_BYTES)];
        Self::Rules(RobotsRules::parse_bounded(&String::from_utf8_lossy(body)))
    }

    /// What a failed robots.txt request or body read means. After a
    /// redirect, a target the crawler may not request leaves no robots.txt
    /// to read (unavailable), and another host failing leaves it
    /// unreachable. `None` when the shop's own host failed, which callers
    /// report as a transport failure.
    pub fn for_failed_request(
        kind: NetworkErrorKind,
        redirected: bool,
        shop_host: bool,
    ) -> Option<Self> {
        match kind {
            _ if !redirected => None,
            NetworkErrorKind::UnsafeTarget => Some(Self::Unavailable),
            _ if shop_host => None,
            _ => Some(Self::Unreachable),
        }
    }

    /// Classifies a non-success, non-redirect robots.txt status. A 429 asks
    /// the crawler to back off, so it counts as unreachable like a 5xx.
    pub fn for_failed_status(status: u16) -> Self {
        if status >= 500 || status == 429 {
            Self::Unreachable
        } else {
            Self::Unavailable
        }
    }

    pub fn check(&self, url: &Url) -> Result<(), RobotsRefusal> {
        match self {
            Self::Rules(rules) if !rules.allows(url) => Err(RobotsRefusal::Disallowed),
            Self::Rules(_) | Self::Unavailable => Ok(()),
            Self::Unreachable => Err(RobotsRefusal::Unreachable),
        }
    }

    /// Stable label for diagnostics.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Rules(_) => "rules",
            Self::Unavailable => "unavailable_allow_all",
            Self::Unreachable => "unreachable_disallow_all",
        }
    }

    pub fn crawl_delay(&self) -> Option<Duration> {
        match self {
            Self::Rules(rules) => rules.crawl_delay(),
            Self::Unavailable | Self::Unreachable => None,
        }
    }
}

/// The rules of the robots.txt group that applies to the crawler.
#[derive(Debug, Clone, PartialEq)]
pub struct RobotsRules {
    rules: Vec<Rule>,
    crawl_delay: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    allow: bool,
    pattern: String,
}

#[derive(Default)]
struct Group {
    agents: Vec<String>,
    rules: Vec<Rule>,
    crawl_delay: Option<Duration>,
}

impl RobotsRules {
    /// Parses a robots.txt body and keeps the groups that apply to the
    /// crawler: every group naming its product token, otherwise every `*`
    /// group (RFC 9309 section 2.2.1).
    pub fn parse(body: &str) -> Self {
        Self::parse_bounded(truncate_to_limit(body))
    }

    /// [`Self::parse`] for a body already cut to the size limit.
    fn parse_bounded(body: &str) -> Self {
        let groups = parse_groups(body);
        let applies = |token: &str| {
            groups
                .iter()
                .filter(|group| group.agents.iter().any(|agent| agent == token))
                .collect::<Vec<_>>()
        };
        let mut selected = applies(CRAWLER_PRODUCT_TOKEN);
        if selected.is_empty() {
            selected = applies("*");
        }
        Self {
            rules: selected
                .iter()
                .flat_map(|group| group.rules.iter().cloned())
                .collect(),
            crawl_delay: selected.iter().filter_map(|group| group.crawl_delay).max(),
        }
    }

    pub fn crawl_delay(&self) -> Option<Duration> {
        self.crawl_delay
    }

    /// The longest matching rule decides; `Allow` wins a tie and a URL no
    /// rule matches is allowed (RFC 9309 section 2.2.2).
    pub fn allows(&self, url: &Url) -> bool {
        let path = request_path(url);
        if path == "/robots.txt" {
            return true;
        }
        self.rules
            .iter()
            .filter(|rule| pattern_matches(&rule.pattern, &path))
            .max_by_key(|rule| (rule.pattern.len(), rule.allow))
            .is_none_or(|rule| rule.allow)
    }
}

fn truncate_to_limit(body: &str) -> &str {
    if body.len() <= MAX_ROBOTS_TXT_BYTES {
        return body;
    }
    let mut end = MAX_ROBOTS_TXT_BYTES;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    &body[..end]
}

/// Consecutive `User-agent` lines open one group; the first rule record
/// (`Allow`, `Disallow`, `Crawl-delay`) closes the agent list, so the next
/// `User-agent` line starts a new group. Other records, such as `Sitemap`,
/// belong to no group and leave the agent list open, as in Google's
/// reference parser. Lines end in LF, CRLF or CR (RFC 9309 section 2.2), and
/// a leading UTF-8 byte order mark is ignored.
fn parse_groups(body: &str) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let mut collecting_agents = false;

    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    for line in body.split(['\r', '\n']) {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();

        if key == "user-agent" {
            if !collecting_agents {
                groups.push(Group::default());
                collecting_agents = true;
            }
            if let Some(group) = groups.last_mut() {
                group.agents.push(product_token(value));
            }
            continue;
        }
        if !matches!(key.as_str(), "allow" | "disallow" | "crawl-delay") {
            continue;
        }
        collecting_agents = false;
        let Some(group) = groups.last_mut() else {
            continue;
        };
        match key.as_str() {
            "allow" | "disallow" if !value.is_empty() => group.rules.push(Rule {
                allow: key == "allow",
                pattern: normalize_percent_encoding(value),
            }),
            "crawl-delay" => {
                if let Some(delay) = parse_crawl_delay(value) {
                    group.crawl_delay = Some(delay);
                }
            }
            _ => {}
        }
    }
    groups
}

/// `User-agent` values are matched on their product token, case-insensitively,
/// so `AuraHistoriaBot/1.0` addresses the crawler as well.
fn product_token(value: &str) -> String {
    value
        .split(['/', ' '])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn parse_crawl_delay(value: &str) -> Option<Duration> {
    let seconds = value.parse::<f64>().ok()?;
    (seconds.is_finite() && seconds >= 0.0)
        .then(|| Duration::from_secs_f64(seconds.min(MAX_CRAWL_DELAY.as_secs_f64())))
}

/// Brings patterns and request paths into one form: non-ASCII characters
/// percent-encoded and every escape in upper-case hex (RFC 3986 treats
/// `%2b` and `%2B` as equal), so a literal non-ASCII pattern still matches.
fn normalize_percent_encoding(pattern: &str) -> String {
    let mut normalized = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let hex: String = chars.clone().take(2).collect();
            if hex.len() == 2 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                normalized.push('%');
                normalized.push_str(&hex.to_ascii_uppercase());
                chars.nth(1);
                continue;
            }
        }
        if ch.is_ascii() {
            normalized.push(ch);
        } else {
            let mut buffer = [0; 4];
            for byte in ch.encode_utf8(&mut buffer).bytes() {
                normalized.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    normalized
}

fn request_path(url: &Url) -> String {
    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    normalize_percent_encoding(&path)
}

/// Matches a rule pattern against the start of `path`: `*` matches any
/// sequence and a trailing `$` anchors the end.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(pattern) => (pattern, true),
        None => (pattern, false),
    };
    let mut segments = pattern.split('*');
    let first = segments.next().unwrap_or_default();
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };
    let segments: Vec<&str> = segments.collect();
    let Some((last, middle)) = segments.split_last() else {
        return !anchored || rest.is_empty();
    };
    for segment in middle {
        match rest.find(segment) {
            Some(index) => rest = &rest[index + segment.len()..],
            None => return false,
        }
    }
    if anchored {
        rest.ends_with(last)
    } else {
        rest.contains(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(path: &str) -> Url {
        Url::parse(&format!("https://shop.example{path}")).unwrap()
    }

    /// antieksite.nl's robots.txt as served during the 2026-10-15 soak test.
    const ANTIEKSITE: &str = "User-agent: *\n\
        Sitemap: https://www.antieksite.nl/rss/\n\
        Crawl-delay: 30\n\
        \n\
        Content-Signal: ai-train=no, search=yes, ai-input=yes\n\
        User-agent: OAI-SearchBot\n\
        User-agent: GPTBot\n\
        \n\
        Allow: /\n\
        Disallow: /cart/\n\
        \n\
        User-agent: AhrefsBot\n\
        User-agent: SemrushBot\n\
        Disallow: /\n";

    #[test]
    fn should_apply_only_the_wildcard_group_when_no_group_names_the_crawler() {
        let rules = RobotsRules::parse(ANTIEKSITE);

        assert!(rules.allows(&url("/antiek/kast-123")));
        assert!(rules.allows(&url("/cart/")));
        assert_eq!(rules.crawl_delay, Some(Duration::from_secs(30)));
    }

    #[test]
    fn should_keep_the_agent_list_open_across_records_outside_any_group() {
        let rules = RobotsRules::parse(
            "User-agent: AuraHistoriaBot\nSitemap: https://shop.example/sitemap.xml\nUser-agent: *\nDisallow: /\n",
        );

        assert!(!rules.allows(&url("/products/1")));
    }

    #[test]
    fn should_prefer_the_group_naming_the_crawler_over_the_wildcard_group() {
        let rules = RobotsRules::parse(
            "User-agent: *\nDisallow: /\n\nUser-agent: AuraHistoriaBot/2.0\nDisallow: /private\n",
        );

        assert!(rules.allows(&url("/products/1")));
        assert!(!rules.allows(&url("/private/1")));
    }

    #[test]
    fn should_let_the_longest_matching_rule_decide_and_allow_win_ties() {
        let rules = RobotsRules::parse(
            "User-agent: *\nDisallow: /shop\nAllow: /shop/items\nDisallow: /a\nAllow: /a\n",
        );

        assert!(!rules.allows(&url("/shop/cart")));
        assert!(rules.allows(&url("/shop/items/9")));
        assert!(rules.allows(&url("/a/b")));
    }

    #[test]
    fn should_match_rules_as_path_prefixes_with_wildcards_and_end_anchors() {
        let rules = RobotsRules::parse(
            "User-agent: *\nDisallow: /cart\nDisallow: /*?sort=\nDisallow: /*.pdf$\n",
        );

        assert!(rules.allows(&url("/products/cartier-ring")));
        assert!(!rules.allows(&url("/cart/add")));
        assert!(!rules.allows(&url("/category?sort=price")));
        assert!(!rules.allows(&url("/files/catalogue.pdf")));
        assert!(rules.allows(&url("/files/catalogue.pdf.html")));
    }

    #[test]
    fn should_match_percent_encoded_paths_against_literal_patterns() {
        let rules = RobotsRules::parse("User-agent: *\nDisallow: /möbel\n");

        assert!(!rules.allows(&url("/möbel/stuhl")));
    }

    #[test]
    fn should_match_percent_escapes_regardless_of_hex_case() {
        let rules = RobotsRules::parse("User-agent: *\nDisallow: /collections/*%2b*\n");

        assert!(!rules.allows(&url("/collections/a%2bb")));
        assert!(!rules.allows(&url("/collections/a%2Bb")));
        assert!(rules.allows(&url("/collections/a+b")));
    }

    #[test]
    fn should_parse_cr_line_endings_and_a_byte_order_mark() {
        let rules = RobotsRules::parse("\u{feff}User-agent: *\rDisallow: /private\r");

        assert!(!rules.allows(&url("/private/1")));
        assert!(rules.allows(&url("/public")));
    }

    #[test]
    fn should_always_allow_robots_txt_itself() {
        let rules = RobotsRules::parse("User-agent: *\nDisallow: /\n");

        assert!(rules.allows(&url("/robots.txt")));
        assert!(!rules.allows(&url("/")));
    }

    #[test]
    fn should_cap_the_crawl_delay() {
        let rules = RobotsRules::parse("User-agent: *\nCrawl-delay: 3600\n");

        assert_eq!(rules.crawl_delay, Some(MAX_CRAWL_DELAY));
    }

    #[test]
    fn should_allow_everything_when_robots_txt_is_unavailable() {
        let policy = RobotsPolicy::for_failed_status(404);

        assert_eq!(policy, RobotsPolicy::Unavailable);
        assert_eq!(policy.check(&url("/products/1")), Ok(()));
    }

    #[test]
    fn should_disallow_everything_when_robots_txt_is_unreachable() {
        let policy = RobotsPolicy::for_failed_status(503);

        assert_eq!(
            policy.check(&url("/products/1")),
            Err(RobotsRefusal::Unreachable)
        );
    }

    #[test]
    fn should_treat_a_rate_limited_robots_txt_as_unreachable() {
        assert_eq!(
            RobotsPolicy::for_failed_status(429),
            RobotsPolicy::Unreachable
        );
    }

    #[test]
    fn should_keep_rules_within_the_byte_limit_when_invalid_utf8_expands() {
        // Each invalid byte decodes to a three-byte replacement character.
        let mut body = vec![b'#'; 1];
        body.resize(MAX_ROBOTS_TXT_BYTES / 2, 0xE9);
        body.extend_from_slice(b"\nUser-agent: *\nDisallow: /a\n");

        assert_eq!(
            RobotsPolicy::from_body(&body).check(&url("/a")),
            Err(RobotsRefusal::Disallowed)
        );
    }

    #[test]
    fn should_classify_failed_robots_txt_requests() {
        use NetworkErrorKind::{Connect, DnsResolution, UnsafeTarget};

        assert_eq!(
            RobotsPolicy::for_failed_request(Connect, false, false),
            None
        );
        assert_eq!(
            RobotsPolicy::for_failed_request(UnsafeTarget, true, true),
            Some(RobotsPolicy::Unavailable)
        );
        assert_eq!(
            RobotsPolicy::for_failed_request(DnsResolution, true, false),
            Some(RobotsPolicy::Unreachable)
        );
        assert_eq!(RobotsPolicy::for_failed_request(Connect, true, true), None);
    }

    #[test]
    fn should_follow_redirects_with_a_usable_target_only() {
        let robots_url = url("/robots.txt");

        assert_eq!(
            RobotsFetchStep::for_unsuccessful(301, Some("/r/robots.txt"), &robots_url),
            RobotsFetchStep::Redirect(url("/r/robots.txt"))
        );
        assert_eq!(
            RobotsFetchStep::for_unsuccessful(302, None, &robots_url),
            RobotsFetchStep::Policy(RobotsPolicy::Unavailable)
        );
        assert_eq!(
            RobotsFetchStep::for_unsuccessful(404, None, &robots_url),
            RobotsFetchStep::Policy(RobotsPolicy::Unavailable)
        );
    }

    #[test]
    fn should_parse_only_the_first_robots_txt_bytes_of_a_body() {
        let mut body = b"User-agent: *\nDisallow: /a\n".to_vec();
        body.resize(MAX_ROBOTS_TXT_BYTES, b'\n');
        body.extend_from_slice(b"Disallow: /b\n");

        let policy = RobotsPolicy::from_body(&body);

        assert_eq!(policy.check(&url("/a")), Err(RobotsRefusal::Disallowed));
        assert_eq!(policy.check(&url("/b")), Ok(()));
    }
}
