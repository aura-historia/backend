use crate::scraper::css_selector::rule::CssSelector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::page_match::page_classification_matches;

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
