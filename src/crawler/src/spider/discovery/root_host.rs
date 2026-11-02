//! Which of a shop's bare and `www` hosts a crawl runs on
//! (`docs/crawler/networking.md#destination-policy`).
//!
//! A shop configured on one host may serve its root there while every page it links lives on the
//! other one (swedishantiques.biz serves the bare host and links `www.` throughout). Spider follows
//! links on its root's host only, so such a crawl would find nothing. When the root page's links
//! point mostly at the twin host, the preflight moves the crawl root there, as it does when the
//! root redirects to the twin.

use scraper::{Html, Selector};
use std::sync::LazyLock;
use url::{Host, Url};

use crate::spider::utils::url::CrawledUrl;

/// Fewest links to the twin host that move a crawl root there, so a root page with a stray link
/// or two to the twin stays where it is.
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
    /// Links to the twin host on the root page.
    pub twin_links: usize,
    /// Links to the root's own host on the root page, relative links included.
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

/// Where the crawl should run from instead of `root`, judged by the links of the root page `html`:
/// on its twin host when at least [`MIN_TWIN_LINKS`] of them, and more than link the root's own
/// host, point there. Links the crawler never follows ([blacklisted](CrawledUrl::is_blacklisted)
/// ones such as the cart or the account) and links to any other host do not count.
pub(super) fn dominant_twin(root: &Url, html: &[u8]) -> Option<RootReroot> {
    let twin = www_twin(root)?;
    let document = Html::parse_document(&String::from_utf8_lossy(html));
    let (mut root_links, mut twin_links) = (0, 0);
    for link in document.select(&LINKS) {
        let href = link.value().attr("href").unwrap_or_default().trim();
        let Some(url) = root.join(href).ok().map(CrawledUrl::new) else {
            continue;
        };
        if !matches!(url.as_url().scheme(), "http" | "https") || url.is_blacklisted() {
            continue;
        }
        match url.as_url().host_str() {
            Some(host) if Some(host) == root.host_str() => root_links += 1,
            Some(host) if Some(host) == twin.host_str() => twin_links += 1,
            _ => {}
        }
    }
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

    #[test]
    fn should_count_links_to_the_twin_of_the_root_only() {
        // Seen from www., the bare host is the twin and the www. links are the root's own.
        assert_eq!(reroot("https://www.example.com/", 50, 0), None);
        assert_eq!(reroot("https://shop.example.com/", 50, 0), None);
        assert_eq!(reroot("https://192.0.2.1/", 50, 0), None);
    }
}
