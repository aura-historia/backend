use crate::network::policy::NetworkErrorKind;
use crate::scraper::candidate_service::Soft404Fingerprint;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::service::{FetchError, ScraperServiceImpl};
use common::shop_id::ShopId;
use regex::Regex;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

const FINGERPRINT_TTL_DAYS: i64 = 30;
const SOFT_404_SIMILARITY_THRESHOLD: f64 = 0.92;

pub(crate) fn soft_404_probe_urls(url: &Url) -> Vec<Url> {
    let mut probes = Vec::new();
    let marker = format!("aura-soft-404-{}", Uuid::new_v4());
    let fake_number = format!("999999{}", Uuid::new_v4().as_u128() % 1_000_000_000);
    let path_segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if let Some(last_segment) = path_segments.last() {
        if last_segment.chars().any(|c| c.is_ascii_digit())
            && let Ok(digits) = Regex::new(r"\d+")
        {
            let mut segments = path_segments.clone();
            let mutated_last = digits
                .replace_all(last_segment, fake_number.as_str())
                .into_owned();
            segments.pop();
            segments.push(mutated_last);
            push_probe_url(url, &segments, &mut probes);
        }

        let mut slug_segments = path_segments.clone();
        slug_segments.pop();
        slug_segments.push(format!("{last_segment}-{marker}"));
        push_probe_url(url, &slug_segments, &mut probes);

        let mut replacement_segments = path_segments.clone();
        replacement_segments.pop();
        replacement_segments.push(marker);
        push_probe_url(url, &replacement_segments, &mut probes);
    }

    let mut probe = url.clone();
    probe.set_query(None);
    probe.set_fragment(None);
    probe.set_path(&format!("/__aura_soft_404_probe/{}", Uuid::new_v4()));
    probes.push(probe);

    deduplicate_probe_urls(probes)
}

fn push_probe_url(original: &Url, path_segments: &[String], probes: &mut Vec<Url>) {
    let mut probe = original.clone();
    probe.set_query(None);
    probe.set_fragment(None);
    let path = format!("/{}", path_segments.join("/"));
    probe.set_path(&path);
    probes.push(probe);
}

fn deduplicate_probe_urls(probes: Vec<Url>) -> Vec<Url> {
    let mut unique = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for probe in probes {
        if seen.insert(probe.to_string()) {
            unique.push(probe);
        }
    }
    unique
}

pub(crate) fn soft_404_fingerprint(html: &str) -> String {
    let text = stable_visible_text(html);
    let tokens = normalized_tokens(&text);
    tokens.join(" ")
}

pub(crate) fn is_soft_404_match(product_html: &str, fingerprint: &str) -> bool {
    let product_fingerprint = soft_404_fingerprint(product_html);
    fingerprint_similarity(&product_fingerprint, fingerprint) >= SOFT_404_SIMILARITY_THRESHOLD
}

fn is_fingerprint_fresh(fingerprint: &Soft404Fingerprint) -> bool {
    fingerprint.checked_at > OffsetDateTime::now_utc() - time::Duration::days(FINGERPRINT_TTL_DAYS)
}

fn stable_visible_text(html: &str) -> String {
    let mut text = html.to_string();
    for pattern in [
        r"(?is)<script\b[^>]*>.*?</script>",
        r"(?is)<style\b[^>]*>.*?</style>",
        r"(?is)<noscript\b[^>]*>.*?</noscript>",
        r"(?is)<form\b[^>]*>.*?</form>",
        r#"(?is)\b(session|csrf|token|visitor|cookie|phpsessid)[-_a-z0-9]*\s*=\s*["'][^"']*["']"#,
        r#"(?is)\b(session|csrf|token|visitor|cookie|phpsessid)[-_a-z0-9]*=[^&\s<>"']+"#,
    ] {
        if let Ok(regex) = Regex::new(pattern) {
            text = regex.replace_all(&text, " ").into_owned();
        }
    }

    if let Ok(regex) = Regex::new(r"(?is)<[^>]+>") {
        text = regex.replace_all(&text, " ").into_owned();
    }

    text.replace("&amp;", "&")
        .replace("&nbsp;", " ")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
}

fn normalized_tokens(text: &str) -> Vec<String> {
    let mut tokens = text
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| token.len() > 2)
        .filter(|token| !token.chars().all(|c| c.is_ascii_digit()))
        .map(stable_token)
        .collect::<Vec<_>>();
    tokens.sort();
    tokens.dedup();
    tokens
}

fn stable_token(token: &str) -> String {
    if token.len() >= 16 && token.chars().any(|c| c.is_ascii_digit()) {
        let digest = Sha256::digest(token.as_bytes());
        return format!("dyn{:02x}{:02x}", digest[0], digest[1]);
    }
    token.to_string()
}

fn fingerprint_similarity(left: &str, right: &str) -> f64 {
    let left_tokens = left
        .split_whitespace()
        .collect::<std::collections::BTreeSet<_>>();
    let right_tokens = right
        .split_whitespace()
        .collect::<std::collections::BTreeSet<_>>();

    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0.0;
    }

    let intersection = left_tokens.intersection(&right_tokens).count() as f64;
    let union = left_tokens.union(&right_tokens).count() as f64;
    intersection / union
}

impl ScraperServiceImpl {
    pub(crate) async fn soft_404_fingerprint_for(
        &self,
        shop_id: &ShopId,
        url: &Url,
        product_html: &str,
    ) -> Option<String> {
        if !self.soft_404_fingerprints_enabled {
            return None;
        }

        match self
            .candidate_service
            .get_soft_404_fingerprint(shop_id, url)
            .await
        {
            Ok(Some(fingerprint)) if is_fingerprint_fresh(&fingerprint) => {
                return Some(fingerprint.fingerprint);
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    url = %url,
                    "Failed to load soft-404 fingerprint"
                );
                return None;
            }
        }

        for probe_url in soft_404_probe_urls(url) {
            let fetched = match self.html_fetcher.fetch(&probe_url).await {
                Ok(fetched) => fetched,
                Err(FetchError::Network {
                    kind: NetworkErrorKind::HttpStatus(404 | 410),
                    ..
                }) => continue,
                Err(err) => {
                    tracing::debug!(
                        error = ?err,
                        probe_url = %probe_url,
                        "Soft-404 probe fetch failed"
                    );
                    continue;
                }
            };

            let fingerprint = soft_404_fingerprint(&fetched.html);
            if !is_usable_probe_fingerprint(
                url,
                &probe_url,
                &fetched.final_url,
                product_html,
                &fetched.html,
                &fingerprint,
            ) {
                continue;
            }

            if let Err(err) = self
                .candidate_service
                .save_soft_404_fingerprint(shop_id, url, &fingerprint, &probe_url)
                .await
            {
                tracing::warn!(
                    error = %err,
                    url = %url,
                    probe_url = %probe_url,
                    "Failed to save soft-404 fingerprint"
                );
            }

            return Some(fingerprint);
        }

        None
    }
}

fn is_usable_probe_fingerprint(
    original_url: &Url,
    probe_url: &Url,
    final_url: &Url,
    _product_html: &str,
    probe_html: &str,
    fingerprint: &str,
) -> bool {
    if fingerprint.is_empty() || looks_like_block_page(fingerprint) {
        return false;
    }

    if probe_canonicalizes_to_product(original_url, probe_url, final_url, probe_html) {
        return false;
    }

    has_soft_404_marker(probe_html, fingerprint) && !looks_like_product_page(probe_html)
}

fn looks_like_block_page(fingerprint: &str) -> bool {
    let block_terms = [
        "captcha",
        "cloudflare",
        "forbidden",
        "login",
        "robot",
        "security",
        "verify",
    ];
    block_terms
        .iter()
        .filter(|term| fingerprint.contains(**term))
        .count()
        >= 2
}

fn has_soft_404_marker(probe_html: &str, fingerprint: &str) -> bool {
    let raw = probe_html.to_ascii_lowercase();
    let markers = [
        "noindex",
        "404",
        "not found",
        "page missing",
        "could not be found",
        "error_404",
        "seite nicht gefunden",
        "nicht gefunden",
    ];

    markers
        .iter()
        .any(|marker| raw.contains(marker) || fingerprint.contains(marker))
}

fn probe_canonicalizes_to_product(
    original_url: &Url,
    probe_url: &Url,
    final_url: &Url,
    probe_html: &str,
) -> bool {
    if !urls_equivalent(probe_url, final_url) && is_product_like_url(final_url) {
        return true;
    }

    extracted_canonical_url(probe_html)
        .and_then(|raw| resolve_url(original_url, &raw))
        .is_some_and(|url| {
            !urls_equivalent(probe_url, &url)
                && (urls_equivalent(original_url, &url) || is_product_like_url(&url))
        })
        || extracted_og_url(probe_html)
            .and_then(|raw| resolve_url(original_url, &raw))
            .is_some_and(|url| {
                !urls_equivalent(probe_url, &url)
                    && (urls_equivalent(original_url, &url) || is_product_like_url(&url))
            })
}

fn extracted_canonical_url(html: &str) -> Option<String> {
    let tag_regex = Regex::new(r#"(?is)<link\b[^>]*>"#).ok()?;
    tag_regex.find_iter(html).find_map(|tag| {
        let tag = tag.as_str();
        if !tag.to_ascii_lowercase().contains("canonical") {
            return None;
        }
        extract_attribute(tag, "href")
    })
}

fn extracted_og_url(html: &str) -> Option<String> {
    let tag_regex = Regex::new(r#"(?is)<meta\b[^>]*>"#).ok()?;
    tag_regex.find_iter(html).find_map(|tag| {
        let tag = tag.as_str();
        if !tag.to_ascii_lowercase().contains("og:url") {
            return None;
        }
        extract_attribute(tag, "content")
    })
}

fn extract_attribute(tag: &str, attribute: &str) -> Option<String> {
    let href_regex = Regex::new(&format!(r#"(?is)\b{attribute}=["']([^"']+)["']"#)).ok()?;
    href_regex
        .captures(tag)
        .and_then(|captures| captures.get(1))
        .map(|match_| match_.as_str().to_string())
}

fn resolve_url(base: &Url, raw: &str) -> Option<Url> {
    Url::parse(raw).or_else(|_| base.join(raw)).ok()
}

fn urls_equivalent(left: &Url, right: &Url) -> bool {
    comparable_url(left) == comparable_url(right)
}

fn comparable_url(url: &Url) -> String {
    let mut normalized = url.clone();
    normalized.set_query(None);
    normalized.set_fragment(None);
    let path = normalized.path().trim_end_matches('/').to_string();
    if path.is_empty() {
        normalized.set_path("/");
    } else if path != normalized.path() {
        normalized.set_path(&path);
    }
    normalized.to_string()
}

fn is_product_like_url(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    path.contains("/products/")
        || path.contains("/product/")
        || path.contains("/produkt/")
        || path.contains("/itm")
        || path.contains("/item/")
}

fn looks_like_product_page(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    [
        "schema.org/product",
        "\"@type\":\"product\"",
        "\"@type\": \"product\"",
        "property=\"product:",
        "property='product:",
        "productdetailscontroller",
        "pagetype=product",
        "pagetype\"",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        && !has_soft_404_marker(html, &soft_404_fingerprint(html))
}

pub(crate) async fn product_removed_when_soft_404(
    service: &ScraperServiceImpl,
    shop_id: &ShopId,
    url: &Url,
    html: &str,
) -> Result<(), ScraperError> {
    let Some(fingerprint) = service.soft_404_fingerprint_for(shop_id, url, html).await else {
        return Ok(());
    };

    if !is_soft_404_match(html, &fingerprint) {
        return Ok(());
    }

    service.mark_product_removed_best_effort(shop_id, url).await;
    Err(ScraperError::ProductRemoved {
        url: url.clone(),
        details: "product page matched domain soft-404 fingerprint".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_urls() -> (Url, Url) {
        (
            Url::parse("https://www.example.com/product/blue-chair").unwrap(),
            Url::parse("https://www.example.com/product/blue-chair-aura-soft-404-test").unwrap(),
        )
    }

    #[test]
    fn should_generate_numeric_product_probe_for_same_parent_path() {
        let url = Url::parse(
            "https://www.example.com/antique-maps/vintage-sea-chart-map/itm284170?ref=feed#top",
        )
        .unwrap();

        let probes = soft_404_probe_urls(&url);

        assert!(
            probes[0].as_str().starts_with(
                "https://www.example.com/antique-maps/vintage-sea-chart-map/itm999999"
            )
        );
        assert!(probes.iter().all(|probe| probe.query().is_none()));
        assert!(probes.iter().all(|probe| probe.fragment().is_none()));
    }

    #[test]
    fn should_generate_slug_product_probe_for_same_parent_path() {
        let url = Url::parse("https://www.example.com/product/blue-chair").unwrap();

        let probes = soft_404_probe_urls(&url);

        assert!(
            probes[0]
                .as_str()
                .starts_with("https://www.example.com/product/blue-chair-aura-soft-404-")
        );
    }

    #[test]
    fn should_generate_replacement_probe_before_root_fallback_without_child_probe() {
        let url = Url::parse("https://www.example.com/shop/product/blue-chair").unwrap();

        let probes = soft_404_probe_urls(&url);

        assert!(!probes.iter().any(|probe| {
            probe
                .path()
                .starts_with("/shop/product/blue-chair/aura-soft-404-")
        }));
        assert!(
            probes
                .iter()
                .any(|probe| probe.path().starts_with("/shop/product/aura-soft-404-"))
        );
        assert!(
            probes
                .last()
                .is_some_and(|probe| probe.path().starts_with("/__aura_soft_404_probe/"))
        );
    }

    #[test]
    fn should_reject_homepage_like_probe_fingerprint() {
        let product =
            r#"<html><body><main><h1>Blue Chair</h1><p>Available now</p></main></body></html>"#;
        let homepage = r#"
            <html><body><nav>Home Shop About Contact</nav>
            <main><h1>Welcome to our antique shop</h1><p>Browse latest products.</p></main></body></html>
        "#;
        let fingerprint = soft_404_fingerprint(homepage);
        let (original_url, probe_url) = test_urls();

        assert!(!is_usable_probe_fingerprint(
            &original_url,
            &probe_url,
            &probe_url,
            product,
            homepage,
            &fingerprint
        ));
    }

    #[test]
    fn should_reject_homepage_like_probe_even_when_product_matches_it() {
        let homepage = r#"
            <html><body><nav>Home Shop About Contact</nav>
            <main><h1>Welcome to our antique shop</h1><p>Browse latest products.</p></main></body></html>
        "#;
        let fingerprint = soft_404_fingerprint(homepage);
        let (original_url, probe_url) = test_urls();

        assert!(!is_usable_probe_fingerprint(
            &original_url,
            &probe_url,
            &probe_url,
            homepage,
            homepage,
            &fingerprint
        ));
    }

    #[test]
    fn should_reject_login_or_captcha_probe_fingerprint() {
        let product =
            r#"<html><body><main><h1>Blue Chair</h1><p>Available now</p></main></body></html>"#;
        let block = r#"
            <html><body><main><h1>Security check</h1>
            <p>Please verify you are not a robot and complete the captcha login challenge.</p>
            </main></body></html>
        "#;
        let fingerprint = soft_404_fingerprint(block);
        let (original_url, probe_url) = test_urls();

        assert!(!is_usable_probe_fingerprint(
            &original_url,
            &probe_url,
            &probe_url,
            product,
            block,
            &fingerprint
        ));
    }

    #[test]
    fn should_accept_marker_rich_soft_404_probe_fingerprint() {
        let product =
            r#"<html><body><main><h1>Blue Chair</h1><p>Available now</p></main></body></html>"#;
        let original_url = Url::parse("https://www.example.com/product/blue-chair").unwrap();
        let probe_url =
            Url::parse("https://www.example.com/product/blue-chair-aura-soft-404-test").unwrap();
        let soft_404 = r#"
            <html><head><meta name="robots" content="noindex"></head>
            <body><main><h1>404 - Page not found</h1>
            <p>The page could not be found.</p></main></body></html>
        "#;
        let fingerprint = soft_404_fingerprint(soft_404);

        assert!(is_usable_probe_fingerprint(
            &original_url,
            &probe_url,
            &probe_url,
            product,
            soft_404,
            &fingerprint
        ));
    }

    #[test]
    fn should_reject_probe_canonicalized_to_original_product() {
        let original_url =
            Url::parse("https://www.drewpritchard.co.uk/products/pair-of-desk-lights").unwrap();
        let probe_url = Url::parse(
            "https://www.drewpritchard.co.uk/products/pair-of-desk-lights-aura-soft-404-test",
        )
        .unwrap();
        let html = r#"
            <html><head>
            <link rel="canonical" href="https://www.drewpritchard.co.uk/products/pair-of-desk-lights">
            <meta property="og:url" content="https://www.drewpritchard.co.uk/products/pair-of-desk-lights">
            <title>A Pair of Bronzed Counter Lamps</title>
            </head><body><script type="application/ld+json">{"@type":"Product"}</script></body></html>
        "#;
        let fingerprint = soft_404_fingerprint(html);

        assert!(!is_usable_probe_fingerprint(
            &original_url,
            &probe_url,
            &probe_url,
            html,
            html,
            &fingerprint
        ));
    }

    #[test]
    fn should_reject_probe_canonicalized_to_original_product_when_href_comes_before_rel() {
        let original_url =
            Url::parse("https://www.drewpritchard.co.uk/products/pair-of-desk-lights").unwrap();
        let probe_url = Url::parse(
            "https://www.drewpritchard.co.uk/products/pair-of-desk-lights-aura-soft-404-test",
        )
        .unwrap();
        let html = r#"
            <html><head>
            <link href="https://www.drewpritchard.co.uk/products/pair-of-desk-lights" rel="canonical">
            <title>A Pair of Bronzed Counter Lamps</title>
            </head><body><main><h1>404 - Page not found</h1></main></body></html>
        "#;
        let fingerprint = soft_404_fingerprint(html);

        assert!(!is_usable_probe_fingerprint(
            &original_url,
            &probe_url,
            &probe_url,
            html,
            html,
            &fingerprint
        ));
    }

    #[test]
    fn should_reject_probe_when_final_url_is_product_like_redirect() {
        let original_url = Url::parse("https://www.example.com/products/blue-chair").unwrap();
        let probe_url =
            Url::parse("https://www.example.com/products/blue-chair-aura-soft-404-test").unwrap();
        let final_url = Url::parse("https://www.example.com/products/blue-chair").unwrap();
        let html = r#"
            <html><body><main><h1>404 - Page not found</h1>
            <p>The page could not be found.</p></main></body></html>
        "#;
        let fingerprint = soft_404_fingerprint(html);

        assert!(!is_usable_probe_fingerprint(
            &original_url,
            &probe_url,
            &final_url,
            html,
            html,
            &fingerprint
        ));
    }

    #[test]
    fn should_reject_product_schema_probe_without_soft_404_marker() {
        let original_url = Url::parse("https://www.example.com/products/blue-chair").unwrap();
        let probe_url =
            Url::parse("https://www.example.com/products/blue-chair-aura-soft-404-test").unwrap();
        let html = r#"
            <html><body><script type="application/ld+json">{"@type":"Product","name":"Blue Chair"}</script>
            <main><h1>Blue Chair</h1><p>Available now</p></main></body></html>
        "#;
        let fingerprint = soft_404_fingerprint(html);

        assert!(!is_usable_probe_fingerprint(
            &original_url,
            &probe_url,
            &probe_url,
            html,
            html,
            &fingerprint
        ));
    }

    #[test]
    fn should_match_soft_404_with_changed_session_noise() {
        let first = r#"
            <html><head><title>404</title></head>
            <body><script>session='abc1234567899999'</script>
            <main><h1>Sorry, the page you're looking for couldn't be found</h1>
            <p>Try searching for antiques using the search bar below.</p>
            <section><h2>Latest items</h2><p>20,947 items</p></section></main></body></html>
        "#;
        let second = first
            .replace("abc1234567899999", "zzz9876543210000")
            .replace("20,947", "21,110");

        let fingerprint = soft_404_fingerprint(first);

        assert!(is_soft_404_match(&second, &fingerprint));
    }

    #[test]
    fn should_not_match_product_page_to_soft_404() {
        let soft_404 = r#"
            <html><body><main><h1>Sorry, the page you're looking for couldn't be found</h1>
            <p>Try searching for antiques using the search bar below.</p></main></body></html>
        "#;
        let product = r#"
            <html><body><main><h1>Victorian Sea Chart Map</h1>
            <span>Available</span><img src="/map.jpg"></main></body></html>
        "#;

        let fingerprint = soft_404_fingerprint(soft_404);

        assert!(!is_soft_404_match(product, &fingerprint));
    }

    #[test]
    fn should_match_soft_404_pages_for_different_shop_templates() {
        let templates = [
            (
                r#"
                    <html><head><title>Seite nicht gefunden</title></head>
                    <body><nav>Militaria Orden Antiquitaeten Ankauf Kontakt</nav>
                    <main><h1>Die gesuchte Seite wurde nicht gefunden</h1>
                    <p>Bitte nutzen Sie die Suche oder gehen Sie zur Startseite.</p></main></body></html>
                "#,
                r#"
                    <html><head><title>Seite nicht gefunden</title></head>
                    <body><nav>Militaria Orden Antiquitaeten Ankauf Kontakt</nav>
                    <main><h1>Die gesuchte Seite wurde nicht gefunden</h1>
                    <p>Bitte nutzen Sie die Suche oder gehen Sie zur Startseite.</p></main></body></html>
                "#,
            ),
            (
                r#"
                    <html><head><title>Page Not Found | Shop</title></head>
                    <body><header>Vintage Furniture Decor Lighting Art</header>
                    <main><h1>Oops! We can't find that page.</h1>
                    <p>The item may have moved or is no longer available.</p></main></body></html>
                "#,
                r#"
                    <html><head><title>Page Not Found | Shop</title></head>
                    <body><header>Vintage Furniture Decor Lighting Art</header>
                    <main><h1>Oops! We can't find that page.</h1>
                    <p>The item may have moved or is no longer available.</p></main></body></html>
                "#,
            ),
            (
                r#"
                    <html><head><title>404</title></head>
                    <body><main><h1>Diese Seite konnte nicht gefunden werden.</h1>
                    <p>Es wurden keine Produkte gefunden, die deiner Auswahl entsprechen.</p></main>
                    <footer>Impressum Datenschutz Versand Zahlungsarten</footer></body></html>
                "#,
                r#"
                    <html><head><title>404</title></head>
                    <body><main><h1>Diese Seite konnte nicht gefunden werden.</h1>
                    <p>Es wurden keine Produkte gefunden, die deiner Auswahl entsprechen.</p></main>
                    <footer>Impressum Datenschutz Versand Zahlungsarten</footer></body></html>
                "#,
            ),
        ];

        for (probe_html, product_html) in templates {
            let fingerprint = soft_404_fingerprint(probe_html);

            assert!(is_soft_404_match(product_html, &fingerprint));
        }
    }

    #[test]
    fn should_not_match_real_shop_product_fixtures_to_soft_404_fingerprint() {
        let soft_404 = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/html/antiquesboutique_removed.html"
        ));
        let products = [
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/html/weitze_available.html"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/html/chairish_available.html"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/html/lot-tissimo_listed.html"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/html/antik-und-stil_available.html"
            )),
        ];
        let fingerprint = soft_404_fingerprint(soft_404);

        for product_html in products {
            assert!(!is_soft_404_match(product_html, &fingerprint));
        }
    }
}
