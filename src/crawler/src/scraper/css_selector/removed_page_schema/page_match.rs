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
    values
        .iter()
        .any(|value| page_classification_value_contains_text(value, text))
}

pub(super) fn page_classification_value_contains_text(value: &str, text: &str) -> bool {
    let expected = normalize_page_classification_text(text);
    !expected.is_empty() && normalize_page_classification_text(value).contains(&expected)
}

pub(super) fn page_classification_regex_matches(
    selector: &CssSelector,
    pattern: &str,
    html: &str,
) -> bool {
    let Some(regex) = page_classification_regex(pattern) else {
        return false;
    };
    let Some(values) = selected_texts(selector, html) else {
        return false;
    };

    values
        .iter()
        .any(|value| regex.is_match(&normalize_page_classification_text(value)))
}

pub(super) fn page_classification_value_matches_regex(value: &str, pattern: &str) -> bool {
    page_classification_regex(pattern)
        .is_some_and(|regex| regex.is_match(&normalize_page_classification_text(value)))
}

fn page_classification_regex(pattern: &str) -> Option<Regex> {
    if pattern.trim().is_empty() {
        return None;
    }
    Regex::new(pattern).ok()
}
