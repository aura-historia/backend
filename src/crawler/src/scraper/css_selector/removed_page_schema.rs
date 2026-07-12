use crate::scraper::css_selector::rule::{CssSelector, ExtractionCardinality, ExtractionKind};
use common::shop_id::ShopId;
use schemars::JsonSchema;
use scraper::Html;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShopsRemovedPageSchema {
    pub shop_id: ShopId,
    pub removed_page_schemas: Vec<RemovedPageSchema>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemovedPageSchema {
    pub selector: CssSelector,
    pub text: String,
}

impl RemovedPageSchema {
    pub fn matches(&self, html: &str) -> bool {
        page_classification_matches(&self.selector, &self.text, html)
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NonProductPageSchema {
    pub selector: CssSelector,
    pub text: String,
}

impl NonProductPageSchema {
    pub fn matches(&self, html: &str) -> bool {
        page_classification_matches(&self.selector, &self.text, html)
    }
}

fn normalize_removed_page_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn page_classification_matches(selector: &CssSelector, text: &str, html: &str) -> bool {
    let parsed = Html::parse_document(html);
    let rule = crate::scraper::css_selector::rule::ExtractionRule {
        selector: selector.clone(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::All,
    };

    let Ok(values) = rule.apply(&parsed) else {
        return false;
    };
    let expected = normalize_removed_page_text(text);
    if expected.is_empty() {
        return false;
    }

    values
        .iter()
        .map(|value| normalize_removed_page_text(value))
        .any(|value| value.contains(&expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> RemovedPageSchema {
        RemovedPageSchema {
            selector: CssSelector::from("#mainCatCol h1"),
            text: "Sorry, the page you're looking for couldn't be found".to_string(),
        }
    }

    #[test]
    fn should_match_removed_page_when_selector_and_text_match() {
        let html = r#"<main id="mainCatCol"><h1>Sorry, the page you're looking for couldn't be found</h1></main>"#;

        assert!(schema().matches(html));
    }

    #[test]
    fn should_not_match_removed_page_when_text_differs() {
        let html = r#"<main id="mainCatCol"><h1>Vintage Chair</h1></main>"#;

        assert!(!schema().matches(html));
    }

    #[test]
    fn should_not_match_removed_page_when_text_exists_outside_selector() {
        let html = r#"
            <div>Sorry, the page you're looking for couldn't be found</div>
            <main id="mainCatCol"><h1>Vintage Chair</h1></main>
        "#;

        assert!(!schema().matches(html));
    }

    #[test]
    fn should_match_removed_page_with_case_and_whitespace_differences() {
        let html = r#"
            <main id="mainCatCol">
                <h1>  Sorry,   the page you're looking for
                    couldn't be found  </h1>
            </main>
        "#;

        assert!(schema().matches(html));
    }
}
