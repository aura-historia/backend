use crate::scraper::css_selector::rule::{
    CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
};
use scraper::Html;

fn normalize_page_classification_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(super) fn page_classification_matches(selector: &CssSelector, text: &str, html: &str) -> bool {
    let parsed = Html::parse_document(html);
    let rule = ExtractionRule {
        selector: selector.clone(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::All,
    };

    let Ok(values) = rule.apply(&parsed) else {
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
