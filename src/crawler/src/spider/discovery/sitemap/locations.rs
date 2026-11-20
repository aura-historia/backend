//! Where a crawl looks for a shop's sitemaps, and in which order it reads the files a sitemap
//! index lists (`docs/crawler/networking.md#sitemaps`).

use url::Url;

use crate::network::policy::is_same_or_www_host;

/// The paths a crawl probes, in order, when robots.txt names no sitemap: the sitemaps.org
/// convention, Yoast's and other plugins' index, and WordPress core's.
const PROBED_PATHS: [&str; 3] = ["/sitemap.xml", "/sitemap_index.xml", "/wp-sitemap.xml"];

/// How the crawl found the sitemaps it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitemapSource {
    /// robots.txt names them in `Sitemap:` records; exactly those are read.
    RobotsTxt,
    /// robots.txt names none, so the conventional paths are probed until one is a sitemap.
    Probe,
}

impl SitemapSource {
    /// Stable label for the crawl diagnostics; not persisted.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RobotsTxt => "robots_txt",
            Self::Probe => "probe",
        }
    }
}

/// The sitemaps a crawl from `root` starts from: the `Sitemap:` records of robots.txt when it
/// has any, resolved against `root` (a record is meant to be absolute; a relative one is read
/// as a path of the shop), else the probed paths on `root`'s origin.
pub fn starting_sitemaps(root: &Url, robots_records: &[String]) -> (SitemapSource, Vec<String>) {
    if robots_records.is_empty() {
        let probes = PROBED_PATHS
            .iter()
            .filter_map(|path| root.join(path).ok())
            .map(String::from)
            .collect();
        return (SitemapSource::Probe, probes);
    }
    (SitemapSource::RobotsTxt, robots_records.to_vec())
}

/// Whether the crawl of `root` may read a sitemap file or list a page at `url`: HTTP(S) on the
/// root's host or its one-`www` twin, on the scheme's default port, without credentials.
pub fn on_shop_host(root: &Url, url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && is_same_or_www_host(root, url)
}

/// Words in a sitemap file's path or query that suggest it lists products, as shop systems and
/// plugins name them (`sitemap_products_1.xml`, `product-sitemap.xml`, `sitemap-items.xml`).
const PRODUCT_WORDS: [&str; 9] = [
    "product", "produkt", "produit", "prodott", "item", "artikel", "article", "stock", "catalog",
];

/// The child sitemaps of an index, those whose name looks like products first, otherwise in the
/// index's order. The order matters only when the request cap stops the crawl before the last
/// child: an index may list dozens of attribute or post sitemaps before its products.
pub fn products_first(children: Vec<Url>) -> Vec<Url> {
    let (products, others): (Vec<_>, Vec<_>) = children.into_iter().partition(looks_like_products);
    products.into_iter().chain(others).collect()
}

fn looks_like_products(url: &Url) -> bool {
    // `sitemap` itself spells `item`.
    let name = format!("{}?{}", url.path(), url.query().unwrap_or_default())
        .to_ascii_lowercase()
        .replace("sitemap", "/");
    PRODUCT_WORDS.iter().any(|word| name.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn should_read_exactly_the_sitemaps_robots_txt_names() {
        let records = vec![
            "https://shop.example/sitemap.php".to_owned(),
            "https://shop.example/googlesitemap/".to_owned(),
        ];

        let (source, sitemaps) = starting_sitemaps(&url("https://shop.example/"), &records);

        assert_eq!(source, SitemapSource::RobotsTxt);
        assert_eq!(sitemaps, records);
    }

    #[test]
    fn should_probe_the_conventional_paths_on_the_roots_origin_without_robots_records() {
        let (source, sitemaps) = starting_sitemaps(&url("https://www.shop.example/en/"), &[]);

        assert_eq!(source, SitemapSource::Probe);
        assert_eq!(
            sitemaps,
            [
                "https://www.shop.example/sitemap.xml",
                "https://www.shop.example/sitemap_index.xml",
                "https://www.shop.example/wp-sitemap.xml"
            ]
        );
    }

    #[test]
    fn should_accept_only_the_roots_host_and_its_www_twin_on_default_ports() {
        let root = url("https://www.shop.example/");

        for accepted in [
            "https://www.shop.example/sitemap.xml",
            "http://shop.example/sitemap.xml",
            "https://WWW.Shop.Example/a",
        ] {
            assert!(on_shop_host(&root, &url(accepted)), "{accepted}");
        }
        for refused in [
            "https://cdn.shop.example/sitemap.xml",
            "https://shop.example.evil.example/sitemap.xml",
            "https://www.shop.example:8443/sitemap.xml",
            "https://user:secret@www.shop.example/sitemap.xml",
            "ftp://www.shop.example/sitemap.xml",
            "https://other.example/sitemap.xml",
        ] {
            assert!(!on_shop_host(&root, &url(refused)), "{refused}");
        }
    }

    #[test]
    fn should_order_product_sitemaps_first_and_keep_the_index_order_otherwise() {
        let children = [
            "https://shop.example/pa_colour-sitemap.xml",
            "https://shop.example/page-sitemap.xml",
            "https://shop.example/product-sitemap.xml",
            "https://shop.example/sitemap_products_1.xml?from=1&to=99",
            "https://shop.example/post-sitemap.xml",
            "https://shop.example/product-sitemap2.xml",
        ]
        .map(url)
        .to_vec();

        let ordered = products_first(children);

        assert_eq!(
            ordered.iter().map(Url::as_str).collect::<Vec<_>>(),
            [
                "https://shop.example/product-sitemap.xml",
                "https://shop.example/sitemap_products_1.xml?from=1&to=99",
                "https://shop.example/product-sitemap2.xml",
                "https://shop.example/pa_colour-sitemap.xml",
                "https://shop.example/page-sitemap.xml",
                "https://shop.example/post-sitemap.xml",
            ]
        );
    }
}
