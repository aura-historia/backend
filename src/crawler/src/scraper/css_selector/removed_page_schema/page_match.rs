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

/// Normalized selector texts that match `pattern`.
pub(super) fn page_classification_regex_matches(
    selector: &CssSelector,
    pattern: &str,
    html: &Html,
) -> Vec<String> {
    if pattern.trim().is_empty() {
        return Vec::new();
    }
    let Ok(regex) = Regex::new(pattern) else {
        return Vec::new();
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
