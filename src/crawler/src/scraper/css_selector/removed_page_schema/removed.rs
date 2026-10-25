use crate::scraper::css_selector::rule::CssSelector;
use regex::Regex;
use schemars::JsonSchema;
use scraper::Html;
use serde::{Deserialize, Serialize};

use super::page_match::{
    normalize_page_classification_text, normalized_selected_texts, removed_page_regex,
};

/// The removed-page text or compiled regex a schema looks for, prepared once per
/// schema so that matching pages and dismissing candidates share it.
#[derive(Debug, Clone)]
enum NoticeSignal {
    /// Normalized text the selected page text must contain.
    Text(String),
    Regex(Regex),
}

impl NoticeSignal {
    fn matches(&self, normalized_value: &str) -> bool {
        match self {
            Self::Text(expected) => normalized_value.contains(expected),
            Self::Regex(regex) => regex.is_match(normalized_value),
        }
    }
}

/// A stored removed-page schema together with the normalized page texts it matched.
#[derive(Debug, Clone)]
pub struct RemovedPageMatch {
    pub schema: RemovedPageSchema,
    pub notices: Vec<String>,
    signal: NoticeSignal,
}

impl RemovedPageMatch {
    /// Returns whether an extracted value is the matched removed-page notice rather
    /// than product data: it equals a matched text, or it carries the removed-page
    /// text or regex itself and sits inside a matched text because the selector
    /// picked a container around the notice heading. [`removed_page_regex`] rejects
    /// patterns that match any text, so a stored regex cannot dismiss every product
    /// title.
    pub fn is_notice(&self, value: &str) -> bool {
        let value = normalize_page_classification_text(value);
        if self.notices.contains(&value) {
            return true;
        }
        self.signal.matches(&value) && self.notices.iter().any(|notice| notice.contains(&value))
    }
}

/// Appends each of `added` that `stored` does not hold yet, keeping the stored order, so a
/// ListingSource stores each removed-page template once whichever path proposes it.
pub fn extend_with_new_removed_page_schemas(
    stored: &mut Vec<RemovedPageSchema>,
    added: impl IntoIterator<Item = RemovedPageSchema>,
) {
    for schema in added {
        if !stored.contains(&schema) {
            stored.push(schema);
        }
    }
}

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
        !self.matched_notices(html).is_empty()
    }

    /// Returns the normalized texts selected by this schema that carry its removed-page
    /// text or regex; empty when the page does not match.
    pub fn matched_notices(&self, html: &str) -> Vec<String> {
        self.matched_notices_in(&Html::parse_document(html))
    }

    /// Same as [`Self::matched_notices`] on an already parsed document, so one parse
    /// serves every stored schema.
    pub fn matched_notices_in(&self, html: &Html) -> Vec<String> {
        self.notice_signal()
            .map(|signal| self.notices_for(&signal, html))
            .unwrap_or_default()
    }

    /// Returns this schema with its matched notices, or `None` when the page does not
    /// match.
    pub fn match_page(self, html: &Html) -> Option<RemovedPageMatch> {
        let signal = self.notice_signal()?;
        let notices = self.notices_for(&signal, html);
        if notices.is_empty() {
            return None;
        }
        Some(RemovedPageMatch {
            schema: self,
            notices,
            signal,
        })
    }

    fn notices_for(&self, signal: &NoticeSignal, html: &Html) -> Vec<String> {
        normalized_selected_texts(&self.selector, html)
            .into_iter()
            .filter(|value| signal.matches(value))
            .collect()
    }

    /// Prepares the schema's text or regex for matching. A stored row whose regex
    /// is rejected (the rule may have tightened since it was saved) is logged and
    /// yields no signal; without the warning the source would silently lose
    /// soft-404 detection.
    fn notice_signal(&self) -> Option<NoticeSignal> {
        match (self.text.as_deref(), self.regex.as_deref()) {
            (Some(text), None) => {
                let expected = normalize_page_classification_text(text);
                (!expected.is_empty()).then_some(NoticeSignal::Text(expected))
            }
            (None, Some(pattern)) => match removed_page_regex(pattern) {
                Ok(regex) => Some(NoticeSignal::Regex(regex)),
                Err(reason) => {
                    tracing::warn!(
                        removed_page_selector = %self.selector,
                        removed_page_regex_rejection = %reason,
                        "Stored removed-page regex is rejected and ignored; fix or delete the row"
                    );
                    None
                }
            },
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match (self.text.as_deref(), self.regex.as_deref()) {
            (Some(text), None) if !text.trim().is_empty() => Ok(()),
            (None, Some(pattern)) => removed_page_regex(pattern).map(|_| ()),
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
    fn should_return_only_selected_texts_that_carry_the_removed_page_signal() {
        let html = r#"<main id="mainCatCol">
            <h1>  Sorry, the page you're looking for
                couldn't be found </h1><h1>Related items</h1></main>"#;

        let matched = schema()
            .match_page(&Html::parse_document(html))
            .expect("page matches");

        assert_eq!(
            matched.notices,
            vec!["sorry, the page you're looking for couldn't be found".to_string()]
        );
        assert!(matched.is_notice("SORRY, the page you're looking for couldn't be found"));
        assert!(!matched.is_notice("Biedermeier Chair"));
    }

    #[test]
    fn should_treat_title_inside_container_notice_as_notice_when_it_carries_the_text() {
        let html = r#"<main id="mainCatCol">
            <h1>Sorry, the page you're looking for couldn't be found</h1>
            <div class="related"><h2>Biedermeier Chair</h2></div></main>"#;
        let container = RemovedPageSchema {
            selector: CssSelector::from("#mainCatCol"),
            text: Some("couldn't be found".to_string()),
            regex: None,
        };

        let matched = container
            .match_page(&Html::parse_document(html))
            .expect("page matches");

        assert!(matched.is_notice("Sorry, the page you're looking for couldn't be found"));
        assert!(!matched.is_notice("Biedermeier Chair"));
    }

    #[test]
    fn should_treat_title_inside_container_notice_as_notice_when_it_matches_the_regex() {
        let html = r#"<main id="mainCatCol">
            <h1>Sorry, the page you're looking for couldn't be found</h1>
            <div class="related"><h2>Biedermeier Chair</h2></div></main>"#;
        let container = RemovedPageSchema {
            selector: CssSelector::from("#mainCatCol"),
            text: None,
            regex: Some(r"couldn't be found".to_string()),
        };

        let matched = container
            .match_page(&Html::parse_document(html))
            .expect("page matches");

        assert!(matched.is_notice("Sorry, the page you're looking for couldn't be found"));
        assert!(!matched.is_notice("Biedermeier Chair"));
    }

    #[test]
    fn should_reject_blank_regex_with_the_regex_specific_message() {
        let err = regex_schema("   ")
            .validate()
            .expect_err("blank regex is rejected");

        assert_eq!(err, "Removed page regex must not be empty");
    }

    #[test]
    fn should_not_match_page_when_regex_matches_any_text() {
        let html = r#"<body><h1>Biedermeier Chair</h1><p>In stock</p></body>"#;
        let parsed = Html::parse_document(html);

        for pattern in [
            ".*",
            ".+",
            r"\w+",
            "(?s).*",
            "^.{5,}$",
            "(?i)^[a-z].*",
            "[A-Z].*",
            r"\d+",
            r"[^\n]{3,}",
        ] {
            let broad = RemovedPageSchema {
                selector: CssSelector::from("h1"),
                text: None,
                regex: Some(pattern.to_string()),
            };

            assert!(broad.validate().is_err(), "{pattern}");
            assert!(broad.clone().match_page(&parsed).is_none(), "{pattern}");
        }
    }

    #[test]
    fn should_not_match_page_when_schema_does_not_apply() {
        let html = r#"<body><h1>Biedermeier Chair</h1></body>"#;

        assert!(schema().match_page(&Html::parse_document(html)).is_none());
    }

    #[test]
    fn should_not_match_removed_page_when_regex_is_invalid() {
        let html = r#"<main id="mainCatCol"><h1>The table from 2020 is not available anymore.</h1></main>"#;

        assert!(!regex_schema("[unclosed").matches(html));
    }
}
