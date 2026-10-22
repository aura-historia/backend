use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorFieldEvaluation {
    pub field: String,
    pub selector: String,
    pub selector_match_count: Option<usize>,
    pub additional_selector_match_counts: Vec<usize>,
    pub error: Option<String>,
    /// Whether a miss of this rule fails the page (`title`, `state`). Set by the
    /// evaluator from the schema field it iterates, never derived from the name.
    #[serde(default)]
    pub required: bool,
    /// Matched elements the `images` rule skipped on this page because they hold no
    /// image URL candidate, such as `href="#"` anchors in a gallery; a count above zero
    /// shows a loose selector. `None` for every other rule, which never skips an element,
    /// and when the `images` rule failed the page, for example by skipping every element.
    #[serde(default)]
    pub skipped_element_count: Option<usize>,
}
