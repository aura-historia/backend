use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::scraper_service::util::html::extract_main_fragment;
use sha2::{Digest, Sha256};

/// Returns the SHA-256 hex digest of the `<main>` tag's inner content, or
/// `None` if no `<main>` tag is present.
pub(crate) fn hash_main_fragment(html: &str) -> Option<String> {
    let content = extract_main_fragment(html)?;
    Some(sha256_hex(content.as_bytes()))
}

/// Returns the SHA-256 hex digest of the full HTML string.
pub(crate) fn hash_html(html: &str) -> String {
    sha256_hex(html.as_bytes())
}

/// Fingerprint the ordered effective schema set and source fallback currency.
/// Schema structures serialize deterministically (raw keys are a `BTreeMap`), so a selector or
/// fallback-currency change forces extraction even for unchanged HTML.
pub(crate) fn fingerprint_scraper_context(
    schemas: &[ProductCssSelectorSchema],
    fallback_currency: Option<money::Currency>,
) -> Result<String, serde_json::Error> {
    let encoded = serde_json::to_vec(&(schemas, fallback_currency.map(money::Currency::as_str)))?;
    Ok(sha256_hex(&encoded))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_hash_main_fragment_when_main_tag_exists() {
        let html = "<html><body><main><h1>Hello</h1></main></body></html>";
        let hash = hash_main_fragment(html).expect("should find <main> tag");

        let mut hasher = Sha256::new();
        hasher.update("<h1>Hello</h1>".as_bytes());
        let expected: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();

        assert_eq!(hash, expected);
    }

    #[test]
    fn should_return_none_from_hash_main_fragment_when_main_tag_missing() {
        let html = "<html><body><section>No main</section></body></html>";
        assert!(hash_main_fragment(html).is_none());
    }

    #[test]
    fn should_hash_full_html_when_main_tag_missing() {
        let html = "<html><body><section>No main</section></body></html>";
        let hash = hash_html(html);

        let mut hasher = Sha256::new();
        hasher.update(html.as_bytes());
        let expected: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();

        assert_eq!(hash, expected);
    }
}
