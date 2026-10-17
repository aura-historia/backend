use crate::scraper::css_selector::rule::{
    CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
};
use regex::Regex;
use scraper::Html;

pub(super) fn normalize_page_classification_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn selected_texts(selector: &CssSelector, html: &Html) -> Option<Vec<String>> {
    let rule = ExtractionRule {
        selector: selector.clone(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::All,
    };

    rule.apply(html).ok()
}

/// Normalized selector texts that contain `text`.
pub(super) fn page_classification_text_matches(
    selector: &CssSelector,
    text: &str,
    html: &Html,
) -> Vec<String> {
    let expected = normalize_page_classification_text(text);
    if expected.is_empty() {
        return Vec::new();
    }

    normalized_selected_texts(selector, html)
        .into_iter()
        .filter(|value| value.contains(&expected))
        .collect()
}

/// Compiles a removed-page regex. Rejects empty and invalid patterns, and patterns
/// that would match any text: such a pattern carries no removed-page signal and,
/// stored on the product title element, would withdraw every live product.
pub(super) fn removed_page_regex(pattern: &str) -> Result<Regex, String> {
    if pattern.trim().is_empty() {
        return Err("Removed page regex must not be empty".to_string());
    }
    let regex =
        Regex::new(pattern).map_err(|err| format!("Removed page regex is invalid: {err}"))?;
    if regex.is_match("") || regex.is_match(OVER_BROAD_REGEX_PROBE) {
        return Err(
            "Removed page regex matches any text and carries no removed-page signal".to_string(),
        );
    }
    Ok(regex)
}

/// Neutral product-like text that no removed-page notice pattern should match.
const OVER_BROAD_REGEX_PROBE: &str = "biedermeier chair 1850 sku 42";

/// Normalized selector texts that match `pattern`.
pub(super) fn page_classification_regex_matches(
    selector: &CssSelector,
    pattern: &str,
    html: &Html,
) -> Vec<String> {
    let regex = match removed_page_regex(pattern) {
        Ok(regex) => regex,
        Err(reason) => {
            // A stored row can fail here after the rule tightened; without a signal the
            // source would silently lose soft-404 detection.
            tracing::warn!(
                removed_page_selector = %selector,
                removed_page_regex_rejection = %reason,
                "Stored removed-page regex is rejected and ignored; fix or delete the row"
            );
            return Vec::new();
        }
    };

    normalized_selected_texts(selector, html)
        .into_iter()
        .filter(|value| regex.is_match(value))
        .collect()
}

fn normalized_selected_texts(selector: &CssSelector, html: &Html) -> Vec<String> {
    selected_texts(selector, html)
        .unwrap_or_default()
        .iter()
        .map(|value| normalize_page_classification_text(value))
        .collect()
}
