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

/// Texts selected by `selector`, with lines separated as product extraction separates
/// them: a matched notice is compared with the titles cached product schemas extract
/// from the same page ([`super::RemovedPageMatch::is_notice`]).
fn selected_texts(selector: &CssSelector, html: &Html) -> Option<Vec<String>> {
    selected_text_rule(selector).apply(html).ok()
}

/// The text rule a removed-page selector is applied as.
pub(super) fn selected_text_rule(selector: &CssSelector) -> ExtractionRule {
    ExtractionRule {
        selector: selector.clone(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::All,
    }
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
    if OVER_BROAD_REGEX_PROBES
        .iter()
        .any(|probe| regex.is_match(probe))
    {
        return Err(
            "Removed page regex matches any text and carries no removed-page signal".to_string(),
        );
    }
    Ok(regex)
}

/// Neutral product-like texts that no removed-page notice pattern should match.
/// They vary in length, case, script and punctuation so that length-only, class-only
/// and anchored catch-all patterns are rejected too. Selected page texts are
/// normalized (whitespace-collapsed, lowercased) before matching, so the probes
/// cover both raw and normalized shapes.
const OVER_BROAD_REGEX_PROBES: [&str; 6] = [
    "",
    "biedermeier chair 1850 sku 42",
    "Biedermeier Chair, 1850 (SKU 42)",
    "élégante commode louis xv – 1 750 €",
    "vase",
    "art-déco lampe nr. 7 / 2nd edition",
];

/// Normalized texts selected by `selector` from the page; empty when the selector
/// does not apply.
pub(super) fn normalized_selected_texts(selector: &CssSelector, html: &Html) -> Vec<String> {
    selected_texts(selector, html)
        .unwrap_or_default()
        .iter()
        .map(|value| normalize_page_classification_text(value))
        .collect()
}
