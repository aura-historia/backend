use crate::scraper::scraper_service::util::html::extract_main_fragment;
use sha2::{Digest, Sha256};

/// Returns the SHA-256 hex digest of the `<main>` tag's inner content, or
/// `None` if no `<main>` tag is present.
pub(crate) fn hash_main_fragment(html: &str) -> Option<String> {
    let content = extract_main_fragment(html)?;
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Returns the SHA-256 hex digest of the full HTML string.
pub(crate) fn hash_html(html: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(html.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
