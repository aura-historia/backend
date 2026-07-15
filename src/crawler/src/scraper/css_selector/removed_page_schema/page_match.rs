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

fn selected_texts(selector: &CssSelector, html: &str) -> Option<Vec<String>> {
    let parsed = Html::parse_document(html);
    let rule = ExtractionRule {
        selector: selector.clone(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::All,
    };

    rule.apply(&parsed).ok()
}

pub(super) fn page_classification_text_matches(
    selector: &CssSelector,
    text: &str,
    html: &str,
) -> bool {
    let Some(values) = selected_texts(selector, html) else {
        return false;
    };
    let expected = normalize_page_classification_text(text);
    if expected.is_empty() {
        return false;
    }

    values
        .iter()
        .map(|value| normalize_page_classification_text(value))
        .any(|value| value.contains(&expected))
}

pub(super) fn page_classification_regex_matches(
    selector: &CssSelector,
    pattern: &str,
    html: &str,
) -> bool {
    if pattern.trim().is_empty() {
        return false;
    }
    let Ok(regex) = Regex::new(pattern) else {
        return false;
    };
    let Some(values) = selected_texts(selector, html) else {
        return false;
    };

    values
        .iter()
        .map(|value| normalize_page_classification_text(value))
        .any(|value| regex.is_match(&value))
}
