//! Which of a shop's bare and `www` hosts a crawl runs on
//! (`docs/crawler/networking.md#destination-policy`).
//!
//! A shop configured on one host may serve its root there while every page it links lives on the
//! other one (swedishantiques.biz serves the bare host and links `www.` throughout). Spider follows
//! links on its root's host only, so such a crawl would find nothing. When the root page links
//! mostly pages on the twin host, the preflight moves the crawl root there, as it does when the
//! root redirects to the twin.

use scraper::{Html, Selector};
use std::collections::HashSet;
use std::sync::LazyLock;
use url::{Host, Url};

use super::page_base::absolute_base;
use crate::spider::utils::url::CrawledUrl;

/// Fewest distinct pages linked on the twin host that move a crawl root there, so a root page
/// with a stray link or two to the twin, however often it repeats them, stays where it is.
pub(super) const MIN_TWIN_LINKS: usize = 5;

static LINKS: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a[href]").expect("link selector"));

/// A crawl root moved to its twin host, with the links on the root page that moved it.
#[derive(Clone, PartialEq, Eq)]
pub struct RootReroot {
    /// The root page the links were counted on.
    pub from: Url,
    /// The same URL on the twin host, which the crawl runs from instead.
    pub to: Url,
    /// Distinct pages the root page links on the twin host.
    pub twin_links: usize,
    /// Distinct pages the root page links on its own host, through relative links too.
    pub root_links: usize,
}

/// The crawl logs the move with this, so it writes each URL as one string in the spelling the
/// crawler stores (`docs/crawler/networking.md#stored-urls`): a root redirect may have brought
/// credentials or a session parameter into the root.
impl std::fmt::Debug for RootReroot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let loggable = |url: &Url| CrawledUrl::new(url.clone()).to_string();
        f.debug_struct("RootReroot")
            .field("from", &loggable(&self.from))
            .field("to", &loggable(&self.to))
            .field("twin_links", &self.twin_links)
            .field("root_links", &self.root_links)
            .finish()
    }
}

/// `url` on its www or bare twin host: `www.<host>` for a bare host, the host without `www.` for a
/// `www.` host; none for an IP address or a `www.` host of one more label.
pub(super) fn www_twin(url: &Url) -> Option<Url> {
    let Some(Host::Domain(host)) = url.host() else {
        return None;
    };
    let twin_host = match host.strip_prefix("www.") {
        Some(bare) if bare.contains('.') => bare.to_owned(),
        Some(_) => return None,
        None => format!("www.{host}"),
    };
    let mut twin = url.clone();
    twin.set_host(Some(&twin_host)).ok()?;
    Some(twin)
}

/// What one page is counted under: its [page spelling](CrawledUrl::page_spelling) without the
/// scheme, in lower case, since the crawl does not fetch a page twice for `http` and `https` or
/// for links that differ only in letter case.
fn page_key(url: &CrawledUrl) -> String {
    let spelling = url.page_spelling();
    let without_scheme = spelling
        .as_str()
        .split_once("://")
        .map_or(spelling.as_str(), |(_, rest)| rest);
    without_scheme.to_ascii_lowercase()
}

/// Where the crawl should run from instead of `root`, judged by the links of the root page `html`:
/// on its twin host when they name at least [`MIN_TWIN_LINKS`] distinct pages there, and more
/// than on the root's own host. A page counts once however often the root page links it, in
/// whichever of its spellings ([`CrawledUrl::page_spelling`]) and under either scheme or letter
/// case ([`page_key`]), so one product in a slider, the menu and the footer does not tip the
/// balance. A link counts where Spider resolves it: against
/// the page's first absolute `<base href>` when it has one, as shops that link `www.` through a
/// `<base>` element and relative links do. Links the crawler never follows
/// ([blacklisted](CrawledUrl::is_blacklisted) ones such as the cart or the account) and links to
/// any other host do not count.
pub(super) fn dominant_twin(root: &Url, html: &[u8]) -> Option<RootReroot> {
    let twin = www_twin(root)?;
    let document = Html::parse_document(&String::from_utf8_lossy(html));
    let base = absolute_base(&document).unwrap_or_else(|| root.clone());
    let (mut root_pages, mut twin_pages) = (HashSet::new(), HashSet::new());
    for link in document.select(&LINKS) {
        let href = link.value().attr("href").unwrap_or_default().trim();
        let Some(url) = base.join(href).ok().map(CrawledUrl::new) else {
            continue;
        };
        if !matches!(url.as_url().scheme(), "http" | "https") || url.is_blacklisted() {
            continue;
        }
        let pages = match url.as_url().host_str() {
            Some(host) if Some(host) == root.host_str() => &mut root_pages,
            Some(host) if Some(host) == twin.host_str() => &mut twin_pages,
            _ => continue,
        };
        pages.insert(page_key(&url));
    }
    let (root_links, twin_links) = (root_pages.len(), twin_pages.len());
    (twin_links >= MIN_TWIN_LINKS && twin_links > root_links).then(|| RootReroot {
        from: root.clone(),
        to: twin,
        twin_links,
        root_links,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn twin_of(url: &str) -> Option<String> {
        www_twin(&Url::parse(url).unwrap()).map(String::from)
    }

    #[test]
    fn should_name_the_www_or_bare_twin_of_a_domain_only() {
        assert_eq!(
            twin_of("https://example.com/shop?x=1").as_deref(),
            Some("https://www.example.com/shop?x=1")
        );
        assert_eq!(
            twin_of("https://www.example.com/").as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(twin_of("https://www.localhost/"), None);
        assert_eq!(twin_of("https://192.0.2.1/"), None);
    }

    /// A root page with `twin` links to `www.example.com`, `own` relative links and links to other
    /// hosts and pages the crawler never follows, none of which count.
    fn root_page(twin: usize, own: usize) -> String {
        let twin_links = (0..twin)
            .map(|i| format!(r#"<a href="https://www.example.com/antiques/{i}">x</a>"#))
            .collect::<String>();
        let own_links = (0..own)
            .map(|i| format!(r#"<a href="/category/{i}">x</a>"#))
            .collect::<String>();
        format!(
            r#"<html><body>{twin_links}{own_links}
               <a href="https://www.example.com/cart">Cart</a>
               <a href="https://shop.example.com/a">Other subdomain</a>
               <a href="https://www.www.example.com/b">Not the twin</a>
               <a href="https://other.example/c">Other domain</a>
               <a href="mailto:shop@example.com">Mail</a></body></html>"#
        )
    }

    fn reroot(root: &str, twin: usize, own: usize) -> Option<RootReroot> {
        dominant_twin(&Url::parse(root).unwrap(), root_page(twin, own).as_bytes())
    }

    #[test]
    fn should_move_the_root_to_its_twin_when_its_links_point_mostly_there() {
        assert_eq!(
            reroot("https://example.com/", MIN_TWIN_LINKS, MIN_TWIN_LINKS - 1),
            Some(RootReroot {
                from: Url::parse("https://example.com/").unwrap(),
                to: Url::parse("https://www.example.com/").unwrap(),
                twin_links: MIN_TWIN_LINKS,
                root_links: MIN_TWIN_LINKS - 1,
            })
        );
    }

    #[test]
    fn should_keep_the_root_below_the_threshold_or_without_a_majority() {
        assert_eq!(reroot("https://example.com/", MIN_TWIN_LINKS - 1, 0), None);
        assert_eq!(
            reroot("https://example.com/", MIN_TWIN_LINKS, MIN_TWIN_LINKS),
            None
        );
        assert_eq!(reroot("https://example.com/", 50, 60), None);
    }

    /// Spellings of one page a shop links it under.
    const SPELLINGS: [&str; 4] = ["", "/", "?utm_source=newsletter", "#top"];

    /// A root page linking `pages` distinct twin pages, each `copies` times in the spelling
    /// `link(origin, i, copy)` gives.
    fn twin_links_counted(
        pages: usize,
        copies: usize,
        link: impl Fn(usize, usize) -> String,
    ) -> Option<usize> {
        let anchors = (0..pages)
            .flat_map(|i| (0..copies).map(move |copy| (i, copy)))
            .map(|(i, copy)| format!(r#"<a href="{}">x</a>"#, link(i, copy)))
            .collect::<String>();
        dominant_twin(
            &Url::parse("https://example.com/").unwrap(),
            format!("<html><body>{anchors}</body></html>").as_bytes(),
        )
        .map(|moved| moved.twin_links)
    }

    #[test]
    fn should_count_a_page_once_whatever_its_scheme_or_letter_case() {
        let spelled = |i: usize, copy: usize| match copy {
            0 => format!("https://www.example.com/antiques/{i}"),
            1 => format!("http://www.example.com/antiques/{i}"),
            2 => format!("https://WWW.Example.com/Antiques/{i}"),
            _ => format!("HTTP://www.example.com/ANTIQUES/{i}"),
        };

        // One below the threshold in distinct pages, however often each is linked.
        assert_eq!(twin_links_counted(MIN_TWIN_LINKS - 1, 4, spelled), None);
        assert_eq!(
            twin_links_counted(MIN_TWIN_LINKS, 4, spelled),
            Some(MIN_TWIN_LINKS)
        );
    }

    #[test]
    fn should_weigh_pages_that_differ_only_in_scheme_or_case_as_one_against_the_roots_own() {
        let page = format!(
            "<html><body>{}{}</body></html>",
            (0..=MIN_TWIN_LINKS)
                .map(|i| format!(r#"<a href="https://www.example.com/a/{i}">x</a>"#))
                .collect::<String>(),
            // One page fewer on the root's host, each linked under four spellings.
            (0..MIN_TWIN_LINKS)
                .flat_map(|i| {
                    [
                        "/b/",
                        "/B/",
                        "http://example.com/b/",
                        "http://EXAMPLE.com/B/",
                    ]
                    .map(|prefix| format!(r#"<a href="{prefix}{i}">x</a>"#))
                })
                .collect::<String>()
        );

        let moved = dominant_twin(
            &Url::parse("https://example.com/").unwrap(),
            page.as_bytes(),
        )
        .map(|moved| (moved.twin_links, moved.root_links));
        assert_eq!(moved, Some((MIN_TWIN_LINKS + 1, MIN_TWIN_LINKS)));
    }

    /// The link counts of the move a root page asks for that links `twin` distinct pages on
    /// `www.example.com` and `own` on the root's host, each page under every spelling of its
    /// side's list.
    fn move_by_pages(
        (twin, twin_spellings): (usize, &[&str]),
        (own, own_spellings): (usize, &[&str]),
    ) -> Option<(usize, usize)> {
        let links = |origin: &str, pages: usize, spellings: &[&str]| {
            (0..pages)
                .flat_map(|i| {
                    spellings.iter().map(move |spelling| {
                        format!(r#"<a href="{origin}/antiques/{i}{spelling}">x</a>"#)
                    })
                })
                .collect::<String>()
        };
        let page = format!(
            "<html><body>{}{}</body></html>",
            links("https://www.example.com", twin, twin_spellings),
            links("", own, own_spellings)
        );
        dominant_twin(
            &Url::parse("https://example.com/").unwrap(),
            page.as_bytes(),
        )
        .map(|moved| (moved.twin_links, moved.root_links))
    }

    #[test]
    fn should_count_a_page_linked_repeatedly_or_in_several_spellings_once() {
        assert_eq!(
            move_by_pages((MIN_TWIN_LINKS - 1, &SPELLINGS), (0, &[""])),
            None
        );
        assert_eq!(
            move_by_pages((MIN_TWIN_LINKS, &SPELLINGS), (0, &[""])),
            Some((MIN_TWIN_LINKS, 0))
        );
    }

    #[test]
    fn should_weigh_the_twin_against_the_root_s_host_by_distinct_pages() {
        // More pages on the twin, though the root's own are linked more often.
        assert_eq!(
            move_by_pages((MIN_TWIN_LINKS + 1, &[""]), (MIN_TWIN_LINKS, &SPELLINGS)),
            Some((MIN_TWIN_LINKS + 1, MIN_TWIN_LINKS))
        );
        // As many pages on either host, though the twin's are linked more often.
        assert_eq!(
            move_by_pages((MIN_TWIN_LINKS, &SPELLINGS), (MIN_TWIN_LINKS, &[""])),
            None
        );
    }

    #[test]
    fn should_log_a_move_compactly_without_credentials_or_session_parameters() {
        let moved = RootReroot {
            from: Url::parse("https://user:secret@example.com/?sessionid=abc&lang=de").unwrap(),
            to: Url::parse("https://user:secret@www.example.com/?sessionid=abc&lang=de").unwrap(),
            twin_links: 7,
            root_links: 2,
        };

        assert_eq!(
            format!("{moved:?}"),
            r#"RootReroot { from: "https://example.com/?lang=de", to: "https://www.example.com/?lang=de", twin_links: 7, root_links: 2 }"#
        );
    }

    /// Spider resolves relative links against the page's `<base>`, so a bare root page whose
    /// `<base>` names `www.` links `www.` throughout.
    #[test]
    fn should_count_relative_links_where_the_page_base_resolves_them() {
        let root = Url::parse("https://example.com/").unwrap();
        let page = |base: &str| {
            let links = (0..MIN_TWIN_LINKS)
                .map(|i| format!(r#"<a href="category/{i}">x</a>"#))
                .collect::<String>();
            format!(r#"<html><head><base href="{base}"></head><body>{links}</body></html>"#)
        };

        assert_eq!(
            dominant_twin(&root, page("https://www.example.com/").as_bytes()).map(|moved| (
                moved.to.to_string(),
                moved.twin_links,
                moved.root_links
            )),
            Some(("https://www.example.com/".to_owned(), MIN_TWIN_LINKS, 0))
        );
        for base in [
            "https://example.com/shop/",
            "/relative/",
            "https://cdn.example/",
        ] {
            assert_eq!(dominant_twin(&root, page(base).as_bytes()), None, "{base}");
        }
    }

    #[test]
    fn should_count_links_to_the_twin_of_the_root_only() {
        // Seen from www., the bare host is the twin and the www. links are the root's own.
        assert_eq!(reroot("https://www.example.com/", 50, 0), None);
        assert_eq!(reroot("https://shop.example.com/", 50, 0), None);
        assert_eq!(reroot("https://192.0.2.1/", 50, 0), None);
    }
}
