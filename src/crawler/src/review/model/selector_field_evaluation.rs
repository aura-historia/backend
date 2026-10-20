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
}
