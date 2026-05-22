use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SelectorFieldEvaluation {
    pub field: String,
    pub selector: String,
    pub selector_match_count: Option<usize>,
    pub additional_selector_match_counts: Vec<usize>,
    pub error: Option<String>,
}
