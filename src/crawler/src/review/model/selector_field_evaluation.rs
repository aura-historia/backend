use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorFieldEvaluation {
    pub field: String,
    pub selector: String,
    /// Elements the primary selector matched, `None` when it does not parse. A price or
    /// estimate text rule counts only elements with visible text, because extraction
    /// takes no value from a blank one.
    pub selector_match_count: Option<usize>,
    /// The same count for each additional selector, `0` for one that does not parse.
    pub additional_selector_match_counts: Vec<usize>,
    pub error: Option<String>,
    /// Whether a miss of this rule fails the page (`title`, `state`). Set by the
    /// evaluator from the schema field it iterates, never derived from the name.
    #[serde(default)]
    pub required: bool,
}
