use common::string_newtype;
use serde::{Deserialize, Serialize};

string_newtype!(CssSelector, serde);
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractionRule {
    pub selector: CssSelector,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_selectors: Vec<CssSelector>,

    #[serde(flatten)]
    pub extract: ExtractionKind,

    #[serde(default)]
    pub cardinality: ExtractionCardinality,
}

string_newtype!(HtmlAttributeName, serde);
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtractionKind {
    Text,
    Attribute { name: HtmlAttributeName },
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionCardinality {
    #[default]
    First,
    All,
}
