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
    /// Matched elements the `images` rule skipped on this page because they hold no
    /// image URL candidate, such as `href="#"` anchors in a gallery; a count above zero
    /// shows a loose selector. `None` for every other rule, which never skips an element,
    /// and when the `images` rule failed the page, for example by skipping every element.
    #[serde(default)]
    pub skipped_element_count: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A matrix cached before the `images` rule skipped elements has no
    /// `skipped_element_count`. It still loads: an `images` rule that applied then had no
    /// element to skip, and one that failed the page reports no count either.
    #[test]
    fn should_load_a_cached_field_evaluation_without_skipped_element_count() {
        let field = serde_json::json!({
            "field": "images",
            "selector": "#gallery a",
            "selector_match_count": 3,
            "additional_selector_match_counts": [],
            "error": null,
            "required": false
        });

        let field = serde_json::from_value::<SelectorFieldEvaluation>(field)
            .expect("a cached field evaluation without the count must load");

        assert_eq!(field.skipped_element_count, None);
    }
}
