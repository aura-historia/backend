use crate::scraper::css_selector::rule::CssSelector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::page_match::page_classification_matches;

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
