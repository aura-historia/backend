//! Lightweight HTML fixture fetcher.
//!
//! Fetches a single URL using the project's own [`ReqwestHtmlFetcher`] (the
//! same browser-impersonating reqwest client used in production) and writes
//! the raw HTML to `tests/fixtures/<shop>/product.html` automatically.
//!
//! The shop name is inferred from the URL's core domain:
//! - `www.weitze.net`      → `weitze`
//! - `shop.example.co.uk`  → `example`
//!
//! Usage:
//! ```text
//! cargo run -p crawler --bin fetch-fixture -- <URL>
//! ```
//!
//! Example:
//! ```text
//! cargo run -p crawler --bin fetch-fixture -- \
//!   "https://www.weitze.net/antiquitaeten/auktionen/Prunkvolles_Biedermeier_Sofa__Mahagoni__1830_1840/405500/"
//! ```
//!
//! Output (relative to the crawler crate root):
//! ```text
//! tests/fixtures/weitze/product.html
//! ```
//!
//! No Postgres, no Gemini API key, no Docker — only an internet connection is
//! required.

use crawler::scraper::scraper_service::{HtmlFetcher, ReqwestHtmlFetcher};
use std::env;
use std::path::PathBuf;
use url::Url;

/// Returns the "core" domain label — the second-level domain name, stripped of
/// any subdomain prefix and TLD suffix.
///
/// Examples:
/// - `www.weitze.net`      → `"weitze"`
/// - `www.example.co.uk`   → `"example"`
/// - `shop.mysite.com`     → `"mysite"`
/// - `example.com`         → `"example"`
fn shop_name_from_host(host: &str) -> &str {
    // Strategy:
    //  1. Split into labels.
    //  2. Count how many trailing labels look like TLD parts (short, all-alpha).
    //     At minimum we consume one TLD label (e.g. "net", "com").
    //     If the second-to-last label is also short all-alpha (e.g. "co"),
    //     we consume that too (handles "co.uk", "com.au", etc.).
    //  3. The label immediately to the left of the TLD block is the SLD.
    let labels: Vec<&str> = host.split('.').filter(|s| !s.is_empty()).collect();

    if labels.len() < 2 {
        return host;
    }

    // Always consume at least one TLD label; consume a second if it is also
    // short+alpha (two-part TLD like "co.uk").
    let mut tld_count = 1usize;
    if labels.len() >= 3 {
        let second_from_right = labels[labels.len() - 2];
        if second_from_right.chars().all(|c| c.is_ascii_alphabetic())
            && second_from_right.len() <= 3
        {
            tld_count = 2;
        }
    }

    let sld_idx = labels.len().saturating_sub(tld_count + 1);
    labels[sld_idx]
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() != 2 {
        eprintln!("Usage: fetch-fixture <URL>");
        eprintln!();
        eprintln!("Example:");
        eprintln!(
            "  cargo run -p crawler --bin fetch-fixture -- \
             \"https://www.weitze.net/antiquitaeten/auktionen/.../405500/\""
        );
        eprintln!();
        eprintln!("The fixture is saved automatically to:");
        eprintln!("  tests/fixtures/<shop>/product.html");
        std::process::exit(1);
    }

    let raw_url = &args[1];

    let url = Url::parse(raw_url).unwrap_or_else(|e| {
        eprintln!("Error: invalid URL '{raw_url}': {e}");
        std::process::exit(1);
    });

    let host = url.host_str().unwrap_or_else(|| {
        eprintln!("Error: URL has no host: {raw_url}");
        std::process::exit(1);
    });

    let shop = shop_name_from_host(host);

    // Resolve output path relative to this crate's root (CARGO_MANIFEST_DIR is
    // set by Cargo at compile time and points to src/crawler/).
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_path = crate_root
        .join("tests")
        .join("fixtures")
        .join(shop)
        .join("product.html");

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!(
                "Error: could not create directory '{}': {e}",
                parent.display()
            );
            std::process::exit(1);
        });
    }

    eprintln!("Shop:     {shop}");
    eprintln!("Fetching: {url}");

    let fetcher = ReqwestHtmlFetcher::new();
    let html = fetcher.fetch(&url).await.unwrap_or_else(|e| {
        eprintln!("Error: fetch failed: {e}");
        std::process::exit(1);
    });

    std::fs::write(&output_path, &html).unwrap_or_else(|e| {
        eprintln!("Error: could not write to '{}': {e}", output_path.display());
        std::process::exit(1);
    });

    eprintln!("Saved {} bytes → {}", html.len(), output_path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shop_name_strips_www_and_tld() {
        assert_eq!(shop_name_from_host("www.weitze.net"), "weitze");
    }

    #[test]
    fn shop_name_strips_subdomain_and_two_part_tld() {
        assert_eq!(shop_name_from_host("shop.example.co.uk"), "example");
    }

    #[test]
    fn shop_name_bare_domain() {
        assert_eq!(shop_name_from_host("example.com"), "example");
    }

    #[test]
    fn shop_name_multi_subdomain() {
        assert_eq!(shop_name_from_host("a.b.mysite.com"), "mysite");
    }
}
