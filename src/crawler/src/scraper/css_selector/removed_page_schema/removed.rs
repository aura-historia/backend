use crate::scraper::css_selector::rule::CssSelector;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::page_match::{page_classification_regex_matches, page_classification_text_matches};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemovedPageSchema {
    pub selector: CssSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

impl RemovedPageSchema {
    pub fn matches(&self, html: &str) -> bool {
        match (self.text.as_deref(), self.regex.as_deref()) {
            (Some(text), None) => page_classification_text_matches(&self.selector, text, html),
            (None, Some(pattern)) => {
                page_classification_regex_matches(&self.selector, pattern, html)
            }
            _ => false,
        }
    }

    pub fn validate_for_llm_response(&self) -> Result<(), String> {
        match (self.text.as_deref(), self.regex.as_deref()) {
            (Some(text), None) if !text.trim().is_empty() => Ok(()),
            (None, Some(pattern)) if !pattern.trim().is_empty() => Regex::new(pattern)
                .map(|_| ())
                .map_err(|err| format!("Removed page regex is invalid: {err}")),
            (Some(_), Some(_)) => {
                Err("Removed page schema must include either text or regex, not both".to_string())
            }
            _ => Err("Removed page schema must include non-empty text or regex".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> RemovedPageSchema {
        RemovedPageSchema {
            selector: CssSelector::from("#mainCatCol h1"),
            text: Some("Sorry, the page you're looking for couldn't be found".to_string()),
            regex: None,
        }
    }

    fn regex_schema(pattern: &str) -> RemovedPageSchema {
        RemovedPageSchema {
            selector: CssSelector::from("#mainCatCol h1"),
            text: None,
            regex: Some(pattern.to_string()),
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

    #[test]
    fn should_match_removed_page_when_selector_regex_matches_variable_text() {
        let html = r#"<main id="mainCatCol"><h1>The table from 2020 is not available anymore.</h1></main>"#;

        assert!(regex_schema(r"the .+ is not available anymore").matches(html));
    }

    #[test]
    fn should_not_match_removed_page_when_selector_regex_does_not_match() {
        let html = r#"<main id="mainCatCol"><h1>Vintage Chair</h1></main>"#;

        assert!(!regex_schema(r"the .+ is not available anymore").matches(html));
    }

    #[test]
    fn should_not_match_removed_page_when_regex_is_invalid() {
        let html = r#"<main id="mainCatCol"><h1>The table from 2020 is not available anymore.</h1></main>"#;

        assert!(!regex_schema("[unclosed").matches(html));
    }
}
